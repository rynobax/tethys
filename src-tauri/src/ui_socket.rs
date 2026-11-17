//! The socket the Electron shell's renderers talk to, one connection per page.
//!
//! Every frame, both ways, is a big-endian `u32` length and then that many
//! bytes, the first of which is the kind: `0` is one JSON object, `1` (to the
//! UI only) is a `u32` channel id followed by raw bytes for that channel.

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use bytes::{BufMut, Bytes, BytesMut};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tracing::{debug, info, warn};

use crate::commands::{self, Ctx};
use crate::error::{AppError, AppResult};
use crate::store::WorkspaceNotifier;
use crate::Daemon;

const KIND_JSON: u8 = 0;
const KIND_BYTES: u8 = 1;
/// A big paste arrives as a JSON array of numbers, several bytes per byte.
const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

/// Quick, and wrong if two for one session swap places: a detach overtaking
/// its attach would leave the subscriber behind.
const ORDERED: &[&str] = &[
    "send_input",
    "resize_session",
    "attach_session",
    "detach_session",
];

/// Unbounded so the PTY reader thread never blocks on a slow renderer.
type Outbox = UnboundedSender<Bytes>;

fn frame(kind: u8, header: &[u8], body: &[u8]) -> Bytes {
    let len = 1 + header.len() + body.len();
    let mut buf = BytesMut::with_capacity(4 + len);
    buf.put_u32(len as u32);
    buf.put_u8(kind);
    buf.put_slice(header);
    buf.put_slice(body);
    buf.freeze()
}

fn json_frame(value: &impl Serialize) -> Option<Bytes> {
    match serde_json::to_vec(value) {
        Ok(body) => Some(frame(KIND_JSON, &[], &body)),
        Err(e) => {
            warn!(error = %e, "UI frame failed to serialize");
            None
        }
    }
}

/// Broadcasts to every connected page.
#[derive(Clone, Default)]
pub struct Events {
    conns: Arc<Mutex<HashMap<u64, Outbox>>>,
}

impl Events {
    pub fn emit(&self, event: &str, payload: impl Serialize) {
        #[derive(Serialize)]
        struct Message<'a, P> {
            event: &'a str,
            payload: P,
        }
        let Some(frame) = json_frame(&Message { event, payload }) else {
            return;
        };
        self.conns
            .lock()
            .unwrap()
            .retain(|_, tx| tx.send(frame.clone()).is_ok());
    }

    fn connect(&self, conn: &Conn) {
        self.conns.lock().unwrap().insert(conn.id, conn.tx.clone());
    }

    fn disconnect(&self, conn_id: u64) {
        self.conns.lock().unwrap().remove(&conn_id);
    }

    #[cfg(test)]
    pub(crate) fn connections(&self) -> usize {
        self.conns.lock().unwrap().len()
    }
}

impl WorkspaceNotifier for Events {
    fn workspace_changed(&self, workspace_id: &str) {
        self.emit("workspace:changed", json!({ "workspace_id": workspace_id }));
    }
}

/// One page's connection.
#[derive(Clone)]
pub struct Conn {
    id: u64,
    tx: Outbox,
}

impl Conn {
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Channel ids are the page's, unique only within its connection.
    pub fn json_channel(&self, channel: u32) -> JsonChannel {
        JsonChannel {
            channel,
            tx: self.tx.clone(),
        }
    }

    pub fn byte_channel(&self, channel: u32) -> ByteChannel {
        ByteChannel {
            conn: self.id,
            channel,
            tx: self.tx.clone(),
        }
    }

    fn reply(&self, id: u32, result: AppResult<Value>) {
        let message = match result {
            Ok(value) => json!({ "id": id, "ok": value }),
            Err(e) => json!({ "id": id, "err": e.to_string() }),
        };
        if let Some(frame) = json_frame(&message) {
            let _ = self.tx.send(frame);
        }
    }
}

pub struct JsonChannel {
    channel: u32,
    tx: Outbox,
}

impl JsonChannel {
    /// `false` once the connection is gone.
    pub fn send(&self, payload: &impl Serialize) -> bool {
        let Some(frame) = json_frame(&json!({ "channel": self.channel, "payload": payload }))
        else {
            return true;
        };
        self.tx.send(frame).is_ok()
    }
}

/// One terminal view's feed of PTY bytes.
pub struct ByteChannel {
    conn: u64,
    channel: u32,
    tx: Outbox,
}

impl ByteChannel {
    /// `false` once the connection is gone.
    pub fn send(&self, data: &[u8]) -> bool {
        self.tx
            .send(frame(KIND_BYTES, &self.channel.to_be_bytes(), data))
            .is_ok()
    }

    pub fn is(&self, conn: u64, channel: u32) -> bool {
        self.conn == conn && self.channel == channel
    }
}

#[derive(Deserialize)]
struct Request {
    id: u32,
    cmd: String,
    #[serde(default)]
    args: Value,
}

/// The caller has already made sure no other Tethys owns the data dir, so a
/// socket file left here is a dead run's.
pub fn bind(path: &Path) -> io::Result<UnixListener> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let listener = UnixListener::bind(path)?;
    info!(path = %path.display(), "ui socket listening");
    Ok(listener)
}

pub async fn serve(listener: UnixListener, daemon: Arc<Daemon>) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(connection(stream, daemon.clone()));
            }
            Err(e) => warn!(error = %e, "ui accept failed"),
        }
    }
}

async fn connection(stream: UnixStream, daemon: Arc<Daemon>) {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let (read, write) = stream.into_split();
    let (tx, rx) = mpsc::unbounded_channel();
    let conn = Conn {
        id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        tx,
    };
    let writer = tokio::spawn(write_frames(write, rx));
    daemon.events.connect(&conn);
    debug!(conn = conn.id, "ui connected");

    // Keystrokes must reach the PTY in the order typed, and a drag's last
    // resize must land last, so PTY commands skip the task-per-request path
    // and go through this one queue.
    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let input = tokio::spawn(apply_in_order(daemon.clone(), conn.clone(), input_rx));

    if let Err(e) = read_requests(read, &daemon, &conn, &input_tx).await {
        warn!(conn = conn.id, error = %e, "ui connection failed");
    }

    daemon.events.disconnect(conn.id);
    input.abort();
    // Drops the receiver, so every channel still holding this connection's
    // sender fails its next send and is let go: PTY subscribers, job feeds.
    writer.abort();
    debug!(conn = conn.id, "ui disconnected");
}

async fn read_requests(
    read: OwnedReadHalf,
    daemon: &Arc<Daemon>,
    conn: &Conn,
    ordered: &UnboundedSender<Request>,
) -> io::Result<()> {
    let mut read = BufReader::new(read);
    loop {
        let len = match read.read_u32().await {
            Ok(len) => len as usize,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        };
        if len == 0 || len > MAX_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("frame length {len} out of bounds"),
            ));
        }
        let mut buf = vec![0u8; len];
        read.read_exact(&mut buf).await?;
        if buf[0] != KIND_JSON {
            warn!(kind = buf[0], "ui sent a frame kind it may not send");
            continue;
        }
        let Some(request) = parse_request(&buf[1..], conn) else {
            continue;
        };
        if ORDERED.contains(&request.cmd.as_str()) {
            let _ = ordered.send(request);
            continue;
        }
        let daemon = daemon.clone();
        let conn = conn.clone();
        tokio::spawn(async move { answer(&daemon, &conn, request).await });
    }
}

/// Answers a request it can't read, if it can at least find the id.
fn parse_request(body: &[u8], conn: &Conn) -> Option<Request> {
    let value: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "ui sent unreadable JSON");
            return None;
        }
    };
    let id = value.get("id").and_then(Value::as_u64);
    match serde_json::from_value::<Request>(value) {
        Ok(request) => Some(request),
        Err(e) => {
            warn!(error = %e, "ui sent a malformed request");
            if let Some(id) = id.and_then(|id| u32::try_from(id).ok()) {
                let e = AppError::Other(format!("malformed request: {e}"));
                conn.reply(id, Err(e));
            }
            None
        }
    }
}

async fn apply_in_order(
    daemon: Arc<Daemon>,
    conn: Conn,
    mut requests: UnboundedReceiver<Request>,
) {
    while let Some(request) = requests.recv().await {
        answer(&daemon, &conn, request).await;
    }
}

async fn answer(daemon: &Daemon, conn: &Conn, request: Request) {
    let Request { id, cmd, args } = request;
    let args = match args {
        Value::Null => Value::Object(Default::default()),
        args => args,
    };
    let result = commands::dispatch(&Ctx { d: daemon, conn }, &cmd, args).await;
    conn.reply(id, result);
}

async fn write_frames(write: OwnedWriteHalf, mut frames: UnboundedReceiver<Bytes>) {
    let mut write = BufWriter::new(write);
    while let Some(frame) = frames.recv().await {
        if write.write_all(&frame).await.is_err() {
            return;
        }
        // Whatever queued up behind it goes out in the same flush.
        while let Ok(frame) = frames.try_recv() {
            if write.write_all(&frame).await.is_err() {
                return;
            }
        }
        if write.flush().await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::agent_bin::AgentBins;
    use crate::artifacts::ArtifactStore;
    use crate::budget::Budgets;
    use crate::github::poller::LiveEvents;
    use crate::github::GithubPoller;
    use crate::inprogress::InProgressWorkspaces;
    use crate::paths::Paths;
    use crate::provision_queue::ProvisionQueue;
    use crate::purge::Purger;
    use crate::registry::{RegistryLoad, Repo, RepoRegistry};
    use crate::sessions::SessionSupervisor;
    use crate::shipping::Shipper;
    use crate::store::Store;

    const WAIT: Duration = Duration::from_secs(10);

    struct Harness {
        _tmp: tempfile::TempDir,
        daemon: Arc<Daemon>,
        socket: PathBuf,
    }

    fn no_registry(root: &Path) -> RegistryLoad {
        RegistryLoad::Missing {
            path: root.join("repos.toml"),
        }
    }

    async fn harness(registry: impl FnOnce(&Path) -> RegistryLoad) -> Harness {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths {
            data_dir: tmp.path().join("data"),
            isolated: true,
        };
        std::fs::create_dir_all(&paths.data_dir).unwrap();
        let registry = Arc::new(registry(tmp.path()));
        let events = Events::default();
        let store = Store::load(paths.state_file(), Box::new(events.clone()))
            .await
            .unwrap();
        let artifacts = Arc::new(ArtifactStore::new(events.clone(), store.clone()));
        let supervisor = Arc::new(SessionSupervisor::new(events.clone(), store.clone()));
        let shipper = Arc::new(Shipper {
            store: store.clone(),
            supervisor: supervisor.clone(),
            tmux_bin: PathBuf::new(),
            agent_bins: AgentBins::default(),
            paths: paths.clone(),
            mcp: None,
        });
        let daemon = Arc::new(Daemon {
            poller: Arc::new(GithubPoller::new(
                store.clone(),
                registry.clone(),
                LiveEvents {
                    events: events.clone(),
                    shipper: shipper.clone(),
                },
            )),
            purger: Arc::new(Purger::new(
                store.clone(),
                paths.clone(),
                registry.clone(),
                events.clone(),
            )),
            budgets: Arc::new(Budgets::new(events.clone())),
            store,
            registry,
            events,
            supervisor,
            artifacts,
            shipper,
            in_progress: InProgressWorkspaces::new(),
            queue: ProvisionQueue::new(),
            agent_bins: AgentBins::default(),
            tmux_bin: PathBuf::new(),
            mcp: None,
            paths,
        });
        let socket = daemon.paths.ui_socket();
        tokio::spawn(serve(bind(&socket).unwrap(), daemon.clone()));
        Harness {
            _tmp: tmp,
            daemon,
            socket,
        }
    }

    #[derive(Debug)]
    enum Frame {
        Json(Value),
        Bytes(u32, Vec<u8>),
    }

    struct Client {
        stream: UnixStream,
        next_id: u32,
    }

    impl Client {
        async fn connect(path: &Path) -> Self {
            Self {
                stream: UnixStream::connect(path).await.unwrap(),
                next_id: 0,
            }
        }

        async fn send_raw(&mut self, message: Value) {
            let body = serde_json::to_vec(&message).unwrap();
            self.stream.write_u32(body.len() as u32 + 1).await.unwrap();
            self.stream.write_u8(KIND_JSON).await.unwrap();
            self.stream.write_all(&body).await.unwrap();
        }

        async fn send(&mut self, cmd: &str, args: Value) -> u32 {
            self.next_id += 1;
            let id = self.next_id;
            self.send_raw(json!({ "id": id, "cmd": cmd, "args": args }))
                .await;
            id
        }

        async fn frame(&mut self) -> Frame {
            tokio::time::timeout(WAIT, async {
                let len = self.stream.read_u32().await.unwrap() as usize;
                let mut buf = vec![0u8; len];
                self.stream.read_exact(&mut buf).await.unwrap();
                match buf[0] {
                    KIND_JSON => Frame::Json(serde_json::from_slice(&buf[1..]).unwrap()),
                    KIND_BYTES => Frame::Bytes(
                        u32::from_be_bytes(buf[1..5].try_into().unwrap()),
                        buf[5..].to_vec(),
                    ),
                    kind => panic!("unknown frame kind {kind}"),
                }
            })
            .await
            .expect("timed out waiting for a frame")
        }

        /// The reply to `id`; every frame before it lands in `seen`.
        async fn reply(&mut self, id: u32, seen: &mut Vec<Frame>) -> Value {
            loop {
                match self.frame().await {
                    Frame::Json(v) if v.get("id") == Some(&json!(id)) => return v,
                    other => seen.push(other),
                }
            }
        }

        async fn call(&mut self, cmd: &str, args: Value) -> Value {
            let id = self.send(cmd, args).await;
            self.reply(id, &mut Vec::new()).await
        }

        async fn bytes(&mut self) -> (u32, Vec<u8>) {
            loop {
                if let Frame::Bytes(channel, data) = self.frame().await {
                    return (channel, data);
                }
            }
        }
    }

    async fn eventually(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + WAIT;
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Bounded so nothing outlives the test: a PTY whose session is forgotten
    /// isn't hung up while its reader thread holds the master.
    fn ticker(supervisor: &SessionSupervisor, id: &str) {
        let script = "i=0; while [ $i -lt 400 ]; do echo tick; i=$((i+1)); sleep 0.01; done";
        supervisor
            .spawn_plain(
                id,
                "ws",
                Path::new("/bin/sh"),
                &["-c".into(), script.into()],
                b"SEED\r\n",
            )
            .unwrap();
    }

    #[tokio::test]
    async fn every_request_gets_one_reply_by_id() {
        let h = harness(no_registry).await;
        let mut c = Client::connect(&h.socket).await;

        assert_eq!(
            c.call("list_workspaces", Value::Null).await,
            json!({ "id": 1, "ok": [] })
        );
        c.send_raw(json!({ "id": 99, "cmd": "list_folders" })).await;
        assert_eq!(
            c.reply(99, &mut Vec::new()).await,
            json!({ "id": 99, "ok": [] })
        );
        assert!(c.call("registry_status", json!({})).await["ok"].is_object());

        let id = c.send("delete_folder", json!({ "id": "nope" })).await;
        assert_eq!(
            c.reply(id, &mut Vec::new()).await,
            json!({ "id": id, "err": "folder not found: nope" })
        );
        let reply = c.call("no_such_command", json!({})).await;
        assert_eq!(reply["err"], "unknown command `no_such_command`");
        let reply = c.call("create_folder", json!({})).await;
        let err = reply["err"].as_str().unwrap();
        assert!(
            err.starts_with("invalid args for command `create_folder`"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn top_level_keys_are_camel_case_and_args_structs_snake_case() {
        let h = harness(no_registry).await;
        let mut c = Client::connect(&h.socket).await;

        let folder = c.call("create_folder", json!({ "name": "Later" })).await["ok"].clone();
        let reply = c
            .call(
                "rename_folder",
                json!({ "args": { "folder_id": folder["id"], "name": "Soon" } }),
            )
            .await;
        assert_eq!(reply["ok"], Value::Null, "{reply}");
        assert_eq!(
            c.call("list_folders", Value::Null).await["ok"][0]["name"],
            "Soon"
        );

        let reply = c
            .call(
                "decide_budget_request",
                json!({ "workspaceId": "ws", "approve": true }),
            )
            .await;
        assert_eq!(reply["err"], "no memory budget request is waiting");
        let reply = c
            .call(
                "decide_budget_request",
                json!({ "workspace_id": "ws", "approve": true }),
            )
            .await;
        let err = reply["err"].as_str().unwrap();
        assert!(err.contains("workspaceId"), "{err}");

        let reply = c.call("get_session", json!({ "workspaceId": "ws" })).await;
        assert_eq!(reply["ok"], Value::Null, "{reply}");
    }

    #[tokio::test]
    async fn events_reach_every_connection() {
        let h = harness(no_registry).await;
        let mut a = Client::connect(&h.socket).await;
        let mut b = Client::connect(&h.socket).await;
        eventually("both connections", || h.daemon.events.connections() == 2).await;

        let id = a
            .send("dismiss_system_error", json!({ "id": "none" }))
            .await;
        let mut seen = Vec::new();
        a.reply(id, &mut seen).await;

        let event = json!({ "event": "system_status:changed", "payload": null });
        assert!(
            seen.iter()
                .any(|f| matches!(f, Frame::Json(v) if *v == event)),
            "{seen:?}"
        );
        match b.frame().await {
            Frame::Json(v) => assert_eq!(v, event),
            other => panic!("expected the event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn job_events_stream_on_the_callers_json_channel() {
        let h = harness(|root| {
            let origin = root.join("origin");
            std::fs::create_dir_all(&origin).unwrap();
            crate::test_support::init_repo_with_commit(&origin);
            let worktree_root = root.join("worktrees");
            std::fs::create_dir_all(&worktree_root).unwrap();
            RegistryLoad::Ok {
                path: root.join("repos.toml"),
                registry: RepoRegistry {
                    worktree_root,
                    repos: vec![Repo {
                        key: "api".into(),
                        remote_url: origin.to_string_lossy().into_owned(),
                        default_branch: None,
                        default_setup_script: None,
                        setup_timeout_secs: Some(30),
                        teardown_script: None,
                        env: Default::default(),
                        copy_files: Vec::new(),
                        claude_notes: None,
                        github_slug: None,
                    }],
                    workspace_doc: None,
                },
            }
        })
        .await;
        let mut c = Client::connect(&h.socket).await;

        let id = c
            .send(
                "create_workspace",
                json!({
                    "args": {
                        "workspace_id": "ws-1",
                        "branch": "feat/x",
                        "repo_selections": ["api"],
                        "agent": "claude",
                        "agent_binary": null,
                        "folder": null,
                    },
                    "onEvent": 42,
                }),
            )
            .await;
        let mut seen = Vec::new();
        let reply = c.reply(id, &mut seen).await;
        assert_eq!(reply["ok"]["id"], "ws-1", "{reply}");

        // The feed is its own task, so it may trail the reply.
        let succeeded = |v: &Value| v["payload"]["kind"] == "success";
        while !seen
            .iter()
            .any(|f| matches!(f, Frame::Json(v) if succeeded(v)))
        {
            seen.push(c.frame().await);
        }
        let feed: Vec<&Value> = seen
            .iter()
            .filter_map(|f| match f {
                Frame::Json(v) if v.get("channel").is_some() => Some(v),
                _ => None,
            })
            .collect();
        assert!(feed.len() > 1, "{feed:?}");
        assert!(feed
            .iter()
            .all(|v| v["channel"] == 42 && v["payload"]["kind"].is_string()));
    }

    #[tokio::test]
    async fn scrollback_is_the_first_frame_on_a_byte_channel() {
        let h = harness(no_registry).await;
        ticker(&h.daemon.supervisor, "s1");
        // So live bytes are flowing, and the ring holds some, at attach.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut c = Client::connect(&h.socket).await;

        let id = c
            .send("attach_session", json!({ "sessionId": "s1", "onBytes": 7 }))
            .await;
        let mut seen = Vec::new();
        assert_eq!(
            c.reply(id, &mut seen).await,
            json!({ "id": id, "ok": null })
        );

        let mut frames = seen.into_iter().filter_map(|f| match f {
            Frame::Bytes(channel, data) => Some((channel, data)),
            Frame::Json(_) => None,
        });
        let (channel, scrollback) = match frames.next() {
            Some(first) => first,
            None => c.bytes().await,
        };
        assert_eq!(channel, 7);
        assert!(scrollback.starts_with(b"SEED\r\ntick"), "{scrollback:?}");
        for _ in 0..3 {
            let (channel, live) = c.bytes().await;
            assert_eq!(channel, 7);
            assert!(!live.windows(4).any(|w| w == b"SEED"), "{live:?}");
        }
        h.daemon.supervisor.forget("s1");
    }

    #[tokio::test]
    async fn input_reaches_the_pty_in_the_order_sent() {
        let h = harness(no_registry).await;
        // `alarm` survives the exec, so a stuck test can't leave `head` behind.
        h.daemon
            .supervisor
            .spawn_plain(
                "s1",
                "ws",
                Path::new("/usr/bin/perl"),
                &["-e".into(), "alarm 30; exec qw(head -n 50)".into()],
                b"",
            )
            .unwrap();
        let mut c = Client::connect(&h.socket).await;
        c.call("attach_session", json!({ "sessionId": "s1", "onBytes": 1 }))
            .await;

        let mut ids = Vec::new();
        for n in 0..50 {
            let data: Vec<u8> = format!("{n}\n").into_bytes();
            ids.push(
                c.send("send_input", json!({ "sessionId": "s1", "data": data }))
                    .await,
            );
        }

        let mut output = Vec::new();
        let mut replies = 0;
        // Echoed as written, then printed back by `head`: each line twice.
        while replies < ids.len() || output.windows(4).filter(|w| w == b"49\r\n").count() < 2 {
            match c.frame().await {
                Frame::Json(v) => {
                    assert_eq!(v["ok"], Value::Null, "{v}");
                    replies += 1;
                }
                Frame::Bytes(_, data) => output.extend(data),
            }
        }
        let text = String::from_utf8_lossy(&output);
        let mut order: Vec<u32> = Vec::new();
        for n in text
            .split(|c: char| !c.is_ascii_digit())
            .filter_map(|s| s.parse().ok())
        {
            if !order.contains(&n) {
                order.push(n);
            }
        }
        assert_eq!(order, (0..50).collect::<Vec<_>>(), "{text}");
        h.daemon.supervisor.forget("s1");
    }

    #[tokio::test]
    async fn a_closed_connection_lets_go_of_what_it_held() {
        let h = harness(no_registry).await;
        ticker(&h.daemon.supervisor, "s1");
        let mut a = Client::connect(&h.socket).await;
        let mut b = Client::connect(&h.socket).await;
        eventually("both connections", || h.daemon.events.connections() == 2).await;
        for c in [&mut a, &mut b] {
            let reply = c
                .call("attach_session", json!({ "sessionId": "s1", "onBytes": 1 }))
                .await;
            assert_eq!(reply["ok"], Value::Null, "{reply}");
        }
        assert_eq!(h.daemon.supervisor.subscriber_count("s1"), 2);

        // Channel ids are per connection: b's detach leaves a's channel 1.
        b.call(
            "detach_session",
            json!({ "sessionId": "s1", "channelId": 1 }),
        )
        .await;
        assert_eq!(h.daemon.supervisor.subscriber_count("s1"), 1);

        drop(a);
        eventually("the closed connection to unregister", || {
            h.daemon.events.connections() == 1
        })
        .await;
        eventually("its PTY subscription to go", || {
            h.daemon.supervisor.subscriber_count("s1") == 0
        })
        .await;
        assert_eq!(b.call("list_folders", Value::Null).await["ok"], json!([]));
        h.daemon.supervisor.forget("s1");
    }
}
