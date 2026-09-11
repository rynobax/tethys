use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use tauri::ipc::{Channel, InvokeResponseBody};
use tracing::{debug, info, warn};

use crate::error::{AppError, AppResult};
use crate::tmux;

const READ_BUF: usize = 4096;

/// Shared scrollback ring buffer handle. Live PTY bytes are appended here
/// (bounded to a per-process capacity) so a fresh `attach` can replay
/// everything emitted before it subscribed.
pub type Ring = Arc<Mutex<VecDeque<u8>>>;

/// Called on the watcher thread once the child has exited *and* its tmux
/// session is confirmed gone — a mere client detach (app shutdown, another
/// client stealing with `-D`) does **not** fire it. The ring is handed in so
/// adapters can scrub trailing tmux chatter before surfacing the exit.
pub type OnExit = Box<dyn FnOnce(Option<i32>, &Ring) + Send>;

/// Called on the reader thread, once, the first time the program turns
/// bracketed paste on — see [`ReadyWatch`].
pub type OnReady = Box<dyn FnOnce() + Send>;

/// What a program writes to turn bracketed paste on (DECSET 2004).
const BRACKETED_PASTE_ON: &[u8] = b"\x1b[?2004h";

/// Inputs to [`PtyProcess::spawn`].
pub struct PtySpawn<'a> {
    /// Program to exec (here, always the tmux binary).
    pub program: &'a Path,
    pub args: &'a [String],
    pub cwd: &'a Path,
    /// Bytes to seed the ring with before the reader thread starts, so the
    /// first attach sees prior scrollback ahead of the live stream.
    pub seed_bytes: &'a [u8],
    pub ring_capacity: usize,
    /// tmux session name, used by the child watcher to tell a true exit
    /// apart from a client detach.
    pub tmux_session_name: String,
    pub tmux_bin: PathBuf,
}

/// The byte-streaming half of a PTY-backed process: spawns a sanitised
/// command on a PTY, maintains a bounded scrollback ring, fans live output
/// out to subscribers, and accepts input/resize. Watches the child for exit
/// and invokes a caller-supplied hook. Supervisors wrap one of these with
/// their own domain metadata.
pub struct PtyProcess {
    master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    ring: Ring,
    /// Fan-out targets for live PTY bytes. Writers that error (client closed)
    /// are dropped on the next tick.
    subscribers: Arc<Mutex<Vec<Channel<InvokeResponseBody>>>>,
    /// Flipped to `false` when the child process exits.
    running: Arc<Mutex<bool>>,
    /// Flipped to `true` once the program has turned bracketed paste on.
    tui_ready: Arc<AtomicBool>,
}

impl PtyProcess {
    /// Open a PTY, exec `program args` in `cwd` with a child-repo-sanitised
    /// environment, and wire up the reader thread, subscriber fan-out, and
    /// child watcher. `on_ready` runs once the program's TUI is up (see
    /// [`ReadyWatch`]); `on_exit` runs only on a true child exit.
    pub fn spawn(req: PtySpawn<'_>, on_ready: OnReady, on_exit: OnExit) -> AppResult<Self> {
        let PtySpawn {
            program,
            args,
            cwd,
            seed_bytes,
            ring_capacity,
            tmux_session_name,
            tmux_bin,
        } = req;

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 30,
                cols: 100,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| AppError::Other(format!("openpty failed: {e}")))?;

        let mut cmd = CommandBuilder::new(program);
        for arg in args {
            cmd.arg(arg);
        }
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        crate::child_env::sanitize_for_child_repo(&mut cmd);

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| AppError::Other(format!("spawn failed: {e}")))?;
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| AppError::Other(format!("clone reader failed: {e}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| AppError::Other(format!("take writer failed: {e}")))?;

        let ring: Ring = Arc::new(Mutex::new(VecDeque::with_capacity(ring_capacity)));
        if !seed_bytes.is_empty() {
            // Seed the ring before the reader thread starts so the first
            // attach sees [seed][tmux's fresh redraw] in that order.
            append_to_ring(&ring, seed_bytes, ring_capacity);
        }
        let subscribers: Arc<Mutex<Vec<Channel<InvokeResponseBody>>>> =
            Arc::new(Mutex::new(Vec::new()));
        let running = Arc::new(Mutex::new(true));
        let tui_ready = Arc::new(AtomicBool::new(false));

        spawn_reader_thread(ReaderThread {
            reader,
            ring: ring.clone(),
            subscribers: subscribers.clone(),
            ring_capacity,
            tui_ready: tui_ready.clone(),
            on_ready,
        });
        spawn_child_watcher(
            child,
            tmux_session_name,
            tmux_bin,
            running.clone(),
            ring.clone(),
            on_exit,
        );

        Ok(Self {
            master: pair.master,
            writer: Arc::new(Mutex::new(writer)),
            ring,
            subscribers,
            running,
            tui_ready,
        })
    }

    /// Register a new output subscriber and return the current scrollback.
    /// The frontend writes the scrollback into xterm first, then drains the
    /// channel for live bytes — zero gap.
    /// Subscribe to live bytes, and return the scrollback to paint first.
    ///
    /// The scrollback has terminal queries stripped out of it
    /// (`scrollback::strip_queries`). A fresh xterm.js answers every question
    /// it reads, and the recorded output contains the ones the program asked
    /// at startup — so replaying it verbatim sends stale replies up the PTY
    /// into whatever the program is showing now. Live bytes are untouched: a
    /// query asked now is one something is waiting on.
    pub fn attach(&self, channel: Channel<InvokeResponseBody>) -> Vec<u8> {
        let raw: Vec<u8> = self.ring.lock().unwrap().iter().copied().collect();
        let scrollback = crate::scrollback::strip_queries(&raw);
        self.subscribers.lock().unwrap().push(channel);
        scrollback
    }

    /// Drop the subscriber with the given channel id. Called when a frontend
    /// pane unmounts. Without this the reader thread keeps fanning bytes to a
    /// channel whose `onmessage` closure still pins the whole xterm instance
    /// (and its scrollback) alive in the webview — the send never errors, so
    /// the retain-on-error path never reclaims it.
    pub fn detach(&self, channel_id: u32) {
        remove_subscriber(&mut self.subscribers.lock().unwrap(), channel_id);
    }

    pub fn send_input(&self, data: &[u8]) -> AppResult<()> {
        let writer = self.writer.clone();
        writer
            .lock()
            .unwrap()
            .write_all(data)
            .map_err(|e| AppError::Other(format!("write: {e}")))?;
        Ok(())
    }

    pub fn resize(&self, cols: u16, rows: u16) -> AppResult<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| AppError::Other(format!("resize: {e}")))?;
        Ok(())
    }

    pub fn is_running(&self) -> bool {
        *self.running.lock().unwrap()
    }

    /// Whether the program has drawn a TUI that will accept a paste. Never
    /// goes back to `false`: a program that turns bracketed paste off again
    /// mid-session is still one that is up.
    pub fn tui_ready(&self) -> bool {
        self.tui_ready.load(Ordering::Acquire)
    }
}

/// Watches the live byte stream for the moment the program is ready for
/// input typed at it — a TUI turning bracketed paste on.
///
/// This is the signal for pasting a draft prompt into a fresh session. It
/// used to be the agent's `SessionStart` hook, which for Claude fires while
/// the TUI is coming up but for codex fires at the *first turn* — so a draft
/// pasted on that signal landed after the user's first message. Both
/// agents' TUIs enable bracketed paste as part of drawing their first frame
/// (codex in its very first bytes), and tmux forwards the mode change to the
/// outer terminal for the active pane, so the outer PTY sees it either way.
///
/// The sequence can straddle two reads, so the tail of each chunk is carried
/// into the next.
struct ReadyWatch {
    carry: Vec<u8>,
    seen: bool,
}

impl ReadyWatch {
    fn new() -> Self {
        Self {
            carry: Vec::new(),
            seen: false,
        }
    }

    /// Feed the next chunk of output. Returns `true` exactly once: on the
    /// chunk in which the enable sequence completes.
    fn observe(&mut self, chunk: &[u8]) -> bool {
        if self.seen {
            return false;
        }
        let mut window = std::mem::take(&mut self.carry);
        window.extend_from_slice(chunk);
        if window
            .windows(BRACKETED_PASTE_ON.len())
            .any(|w| w == BRACKETED_PASTE_ON)
        {
            self.seen = true;
            return true;
        }
        let keep = window.len().min(BRACKETED_PASTE_ON.len() - 1);
        self.carry = window[window.len() - keep..].to_vec();
        false
    }
}

/// Append `data` to the ring, evicting from the front to stay within
/// `capacity`. A write larger than the whole ring keeps only its tail.
pub fn append_to_ring(ring: &Ring, data: &[u8], capacity: usize) {
    let mut ring = ring.lock().unwrap();
    if data.len() >= capacity {
        ring.clear();
        ring.extend(&data[data.len() - capacity..]);
        return;
    }
    let overflow = (ring.len() + data.len()).saturating_sub(capacity);
    for _ in 0..overflow {
        ring.pop_front();
    }
    ring.extend(data.iter().copied());
}

/// Remove any subscriber whose channel id matches `channel_id`.
fn remove_subscriber(subs: &mut Vec<Channel<InvokeResponseBody>>, channel_id: u32) {
    subs.retain(|sub| sub.id() != channel_id);
}

struct ReaderThread {
    reader: Box<dyn Read + Send>,
    ring: Ring,
    subscribers: Arc<Mutex<Vec<Channel<InvokeResponseBody>>>>,
    ring_capacity: usize,
    tui_ready: Arc<AtomicBool>,
    on_ready: OnReady,
}

fn spawn_reader_thread(thread: ReaderThread) {
    let ReaderThread {
        mut reader,
        ring,
        subscribers,
        ring_capacity,
        tui_ready,
        on_ready,
    } = thread;
    std::thread::spawn(move || {
        let mut buf = [0u8; READ_BUF];
        let mut watch = ReadyWatch::new();
        let mut on_ready = Some(on_ready);
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    debug!("pty reader: EOF");
                    break;
                }
                Ok(n) => {
                    let chunk = &buf[..n];
                    append_to_ring(&ring, chunk, ring_capacity);
                    if watch.observe(chunk) {
                        debug!("pty reader: program enabled bracketed paste — TUI is up");
                        tui_ready.store(true, Ordering::Release);
                        if let Some(f) = on_ready.take() {
                            f();
                        }
                    }
                    // Fan out, dropping subscribers whose channel errored.
                    let mut subs = subscribers.lock().unwrap();
                    subs.retain(|sub| {
                        sub.send(InvokeResponseBody::Raw(chunk.to_vec())).is_ok()
                    });
                }
                Err(e) => {
                    warn!(error = %e, "pty reader error");
                    break;
                }
            }
        }
    });
}

fn spawn_child_watcher(
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    tmux_session_name: String,
    tmux_bin: PathBuf,
    running: Arc<Mutex<bool>>,
    ring: Ring,
    on_exit: OnExit,
) {
    std::thread::spawn(move || {
        let status = child.wait();
        *running.lock().unwrap() = false;
        let code = status.ok().map(|s| s.exit_code() as i32);

        // The child here is the tmux *client*. It exits both when the program
        // truly ends (session disappears) and when the client merely detaches
        // (app shutdown, another client steals with -D, etc.). Check
        // has-session to tell them apart.
        if tmux::has_session(&tmux_bin, &tmux_session_name) {
            info!(
                session = %tmux_session_name,
                ?code,
                "tmux client exited but session still alive (detach)"
            );
            return;
        }

        on_exit(code, &ring);
    });
}

#[cfg(test)]
mod tests {
    use super::{append_to_ring, remove_subscriber, ReadyWatch, Ring};
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use tauri::ipc::{Channel, InvokeResponseBody};

    fn ring_bytes(ring: &Ring) -> Vec<u8> {
        ring.lock().unwrap().iter().copied().collect()
    }

    #[test]
    fn remove_subscriber_drops_only_the_matching_channel() {
        let a = Channel::<InvokeResponseBody>::new(|_| Ok(()));
        let b = Channel::<InvokeResponseBody>::new(|_| Ok(()));
        let (a_id, b_id) = (a.id(), b.id());
        let mut subs = vec![a, b];

        remove_subscriber(&mut subs, a_id);

        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].id(), b_id);
    }

    #[test]
    fn remove_subscriber_ignores_unknown_id() {
        let a = Channel::<InvokeResponseBody>::new(|_| Ok(()));
        let mut subs = vec![a];

        remove_subscriber(&mut subs, u32::MAX);

        assert_eq!(subs.len(), 1);
    }

    #[test]
    fn append_within_capacity_keeps_everything() {
        let ring: Ring = Arc::new(Mutex::new(VecDeque::new()));
        append_to_ring(&ring, b"abc", 8);
        append_to_ring(&ring, b"de", 8);
        assert_eq!(ring_bytes(&ring), b"abcde");
    }

    #[test]
    fn append_over_capacity_evicts_from_front() {
        let ring: Ring = Arc::new(Mutex::new(VecDeque::new()));
        append_to_ring(&ring, b"abcd", 4);
        append_to_ring(&ring, b"ef", 4);
        // Oldest two bytes dropped to make room.
        assert_eq!(ring_bytes(&ring), b"cdef");
    }

    #[test]
    fn append_larger_than_capacity_keeps_tail() {
        let ring: Ring = Arc::new(Mutex::new(VecDeque::new()));
        append_to_ring(&ring, b"seed", 4);
        append_to_ring(&ring, b"0123456789", 4);
        assert_eq!(ring_bytes(&ring), b"6789");
    }

    #[test]
    fn ready_fires_once_when_bracketed_paste_is_enabled() {
        let mut w = ReadyWatch::new();
        assert!(!w.observe(b"\x1b[?1049h\x1b[H"));
        assert!(w.observe(b"\x1b[?2004h\x1b[?25l"));
        // Turning it off and on again is not a second readiness.
        assert!(!w.observe(b"\x1b[?2004l\x1b[?2004h"));
    }

    /// A PTY read can end anywhere, including in the middle of the sequence.
    #[test]
    fn ready_survives_a_sequence_split_across_reads() {
        let mut w = ReadyWatch::new();
        assert!(!w.observe(b"hello \x1b[?20"));
        assert!(w.observe(b"04h more"));
    }

    /// Only the enable sequence counts: the query-shaped near-misses a TUI
    /// emits around it must not trip the watch.
    #[test]
    fn ready_ignores_other_private_modes() {
        let mut w = ReadyWatch::new();
        assert!(!w.observe(b"\x1b[?2004l\x1b[?2026h\x1b[?1004h"));
    }
}
