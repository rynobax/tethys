use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tauri::{AppHandle, Emitter};
use tokio::sync::{Mutex, Notify};
use tracing::{info, info_span, trace, warn, Instrument};

use crate::github::client::{gh_login_status, run_graphql, GhCli, GhError, PrSource};
use crate::github::pr_status::{
    apply_results, build_query, parse_response, Applied, PollOutcome, Target, TargetKind,
};
use crate::github::status::GithubPrStatus;
use crate::github::GithubSlug;
use crate::registry::RegistryLoad;
use crate::state::AppState;
use crate::store::Store;

const BASE_INTERVAL: Duration = Duration::from_secs(45);
const MAX_BACKOFF: Duration = Duration::from_secs(600);
const AUTH_RETRY_INTERVAL: Duration = Duration::from_secs(600);
const FORCE_TICK_MIN_SPACING: Duration = Duration::from_secs(10);

/// Runtime state mutated only from within `run()`.
#[derive(Debug, Default)]
struct PollerInner {
    consecutive_failures: u32,
    auth: AuthState,
    login: Option<String>,
    last_forced_tick: Option<Instant>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthState {
    #[default]
    Unknown,
    Authenticated,
    /// `gh` is installed but not logged in. Slow-poll and re-probe.
    NotAuthenticated,
    /// `gh` is missing. Stop polling for the lifetime of the process.
    Disabled,
}

/// Snapshot of auth state returned by the `github_auth_status` Tauri command.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AuthSnapshot {
    pub state: AuthState,
    pub login: Option<String>,
}

/// Where the poller announces what it found.
///
/// One of the two things that stood between this loop and a test — the other
/// being [`PrSource`]. `AppHandle` is a struct field that can't be constructed
/// outside a running Tauri app, so nothing below could ever build a
/// `GithubPoller`, and the scheduling, auth, and fan-out logic had no coverage
/// at all. That is where the "installing gh after boot silently never resumes
/// polling" bug lived.
pub trait PollerEvents: Send + Sync + 'static {
    fn auth_changed(&self, snapshot: &AuthSnapshot);
    fn status_changed(&self, event: &Value);
}

/// Production adapter: emit to the frontend.
pub struct TauriEvents(pub AppHandle);

impl PollerEvents for TauriEvents {
    fn auth_changed(&self, snapshot: &AuthSnapshot) {
        let _ = self.0.emit("github:auth_changed", snapshot);
    }

    fn status_changed(&self, event: &Value) {
        let _ = self.0.emit("github:status_changed", event);
    }
}

pub struct GithubPoller {
    store: Arc<Store>,
    registry: Arc<RegistryLoad>,
    source: Box<dyn PrSource>,
    events: Box<dyn PollerEvents>,
    inner: Mutex<PollerInner>,
    force: Arc<Notify>,
}

impl GithubPoller {
    pub fn new(store: Arc<Store>, registry: Arc<RegistryLoad>, app: AppHandle) -> Self {
        Self::with_seams(
            store,
            registry,
            Box::new(GhCli),
            Box::new(TauriEvents(app)),
        )
    }

    /// Build a poller over explicit adapters. Two of each: `gh` and the Tauri
    /// app in production, fakes in tests.
    pub fn with_seams(
        store: Arc<Store>,
        registry: Arc<RegistryLoad>,
        source: Box<dyn PrSource>,
        events: Box<dyn PollerEvents>,
    ) -> Self {
        Self {
            store,
            registry,
            source,
            events,
            inner: Mutex::new(PollerInner::default()),
            force: Arc::new(Notify::new()),
        }
    }

    /// Long-running loop. Spawn with `tokio::spawn(poller.run())`.
    pub async fn run(self: Arc<Self>) {
        loop {
            let delay = self.next_delay().await;
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = self.force.notified() => {}
            }
            // Disabled means `gh` wasn't installed when we last looked. Park
            // (next_delay is effectively infinite) rather than returning —
            // `probe_login` can flip us back to Authenticated once the user
            // installs it, and it notifies `force` to wake us. Returning here
            // killed the loop for the rest of the process's life while the UI
            // happily reported "authenticated".
            if self.inner.lock().await.auth == AuthState::Disabled {
                continue;
            }
            self.tick().await;
        }
    }

    /// Request an immediate poll tick, bypassing the normal sleep. Rate-limited
    /// so rapid window focus/blur doesn't cause a storm.
    pub async fn request_tick(&self) {
        let mut inner = self.inner.lock().await;
        let now = Instant::now();
        if let Some(prev) = inner.last_forced_tick {
            if now.duration_since(prev) < FORCE_TICK_MIN_SPACING {
                return;
            }
        }
        inner.last_forced_tick = Some(now);
        drop(inner);
        self.force.notify_one();
    }

    async fn next_delay(&self) -> Duration {
        let inner = self.inner.lock().await;
        match inner.auth {
            AuthState::Disabled => Duration::from_secs(u64::MAX),
            AuthState::NotAuthenticated => AUTH_RETRY_INTERVAL,
            _ => {
                let backoff = backoff_for(inner.consecutive_failures);
                let jitter = Duration::from_millis(fastrand::u64(0..5_000));
                backoff + jitter
            }
        }
    }

    async fn tick(self: &Arc<Self>) {
        let targets = self.collect_targets().await;
        if targets.is_empty() {
            return;
        }

        let span = info_span!("github.poll_tick", n_repos = targets.len());
        async {
            let start = std::time::Instant::now();
            match self.do_poll(&targets).await {
                Ok(applied) => {
                    let prev_auth = {
                        let mut inner = self.inner.lock().await;
                        inner.consecutive_failures = 0;
                        let prev = inner.auth.clone();
                        inner.auth = AuthState::Authenticated;
                        prev
                    };
                    if prev_auth != AuthState::Authenticated {
                        self.emit_auth_changed().await;
                    }
                    info!(
                        applied = applied,
                        ms = start.elapsed().as_millis() as u64,
                        "poll tick ok",
                    );
                }
                Err(e) => {
                    let ms = start.elapsed().as_millis() as u64;
                    warn!(ms = ms, error = %e, "poll tick failed");
                    self.handle_error(e).await;
                }
            }
        }
        .instrument(span)
        .await;
    }

    async fn emit_auth_changed(&self) {
        let snap = self.auth_snapshot().await;
        self.events.auth_changed(&snap);
    }

    pub async fn auth_snapshot(&self) -> AuthSnapshot {
        let inner = self.inner.lock().await;
        AuthSnapshot {
            state: inner.auth.clone(),
            login: inner.login.clone(),
        }
    }

    async fn collect_targets(&self) -> Vec<Target> {
        let registry = match self.registry.as_ref() {
            RegistryLoad::Ok { registry, .. } => registry,
            _ => return Vec::new(),
        };

        let snapshot: AppState = self.store.read(|s| s.clone()).await;
        let mut out = Vec::new();
        for ws in snapshot.workspaces {
            for link in &ws.repo_links {
                let Some(repo) = registry.find_repo(&link.repo_key) else {
                    continue;
                };
                let Some(slug) = &repo.github_slug else {
                    continue;
                };
                // One scan for the workspace's own branch, so a PR opened
                // there gets tracked without anyone typing its number...
                out.push(Target {
                    workspace_id: ws.id.clone(),
                    repo_key: link.repo_key.clone(),
                    slug: slug.clone(),
                    kind: TargetKind::Branch(ws.branch.clone()),
                });
                // ...and one status fetch per tracked PR, with nothing here
                // caring how any of them came to be tracked.
                for pr in &link.prs {
                    out.push(Target {
                        workspace_id: ws.id.clone(),
                        repo_key: link.repo_key.clone(),
                        slug: slug.clone(),
                        kind: TargetKind::Pr(pr.number),
                    });
                }
            }
        }
        out
    }

    /// One tick: fetch every target, apply it, then — if the branch scan turned
    /// up a PR nobody was tracking — fetch that PR's status straight away.
    ///
    /// The second pass is what keeps automatic and manual tracking the same
    /// from the outside. Attaching by hand fetches before it records, so a chip
    /// appears fully formed; without this, a PR you had just opened would sit
    /// there saying "no data" until the next tick 45 seconds later.
    async fn do_poll(&self, targets: &[Target]) -> Result<usize, GhError> {
        let applied = self.fetch_and_apply(targets).await?;
        let mut count = applied.changed.len();

        let follow_up: Vec<Target> = applied
            .discovered
            .iter()
            .filter_map(|d| {
                // Reuse the slug the scan itself was built with, so nothing
                // here has to reach back into the registry.
                let slug = targets
                    .iter()
                    .find(|t| t.workspace_id == d.workspace_id && t.repo_key == d.repo_key)
                    .map(|t| t.slug.clone())?;
                Some(Target {
                    workspace_id: d.workspace_id.clone(),
                    repo_key: d.repo_key.clone(),
                    slug,
                    kind: TargetKind::Pr(d.number),
                })
            })
            .collect();

        if !follow_up.is_empty() {
            info!(count = follow_up.len(), "tracking newly discovered PRs");
            // These are all `Pr` targets, so they can't discover anything and
            // this can't cascade past one extra pass.
            let result = self.fetch_and_apply(&follow_up).await;
            // The list itself grew, which no `status_changed` can express — it
            // only ever updates a PR the frontend already has. Announced even
            // when the status fetch failed, so the chip shows up (as "no data")
            // rather than waiting for something else to refresh the workspace.
            for id in dedup(applied.discovered.iter().map(|d| &d.workspace_id)) {
                self.store.notify_workspace_changed(id);
            }
            count += result?.changed.len();
        }
        Ok(count)
    }

    async fn fetch_and_apply(&self, targets: &[Target]) -> Result<Applied, GhError> {
        let (query, variables) = build_query(targets);
        trace!(query = %query, variables = ?variables, "github graphql request");
        let data = self.source.fetch(&query, &variables).await?;
        trace!(data = %data, "github graphql response");
        let parsed = parse_response(targets, &data);

        let applied = self
            .store
            .mutate(|s| Ok(apply_results(s, &parsed)))
            .await
            .unwrap_or_default();

        for result in &applied.changed {
            if let Some(event) = result.event() {
                self.events.status_changed(&event);
            }
        }
        Ok(applied)
    }

    async fn handle_error(&self, err: GhError) {
        let mut auth_changed = false;
        {
            let mut inner = self.inner.lock().await;
            match &err {
                GhError::NotInstalled => {
                    warn!("gh CLI not installed — disabling GitHub polling");
                    if inner.auth != AuthState::Disabled {
                        auth_changed = true;
                    }
                    inner.auth = AuthState::Disabled;
                    inner.login = None;
                }
                GhError::NotAuthenticated => {
                    if inner.auth != AuthState::NotAuthenticated {
                        warn!("gh not authenticated — slowing poll; run `gh auth login`");
                        auth_changed = true;
                    }
                    inner.auth = AuthState::NotAuthenticated;
                    inner.login = None;
                }
                GhError::RateLimited | GhError::Network(_) => {
                    inner.consecutive_failures = inner.consecutive_failures.saturating_add(1);
                    warn!(
                        error = %err,
                        failures = inner.consecutive_failures,
                        "poll tick failed, backing off",
                    );
                }
                GhError::Graphql(msgs) => {
                    inner.consecutive_failures = inner.consecutive_failures.saturating_add(1);
                    warn!(errors = ?msgs, "poll tick returned GraphQL errors");
                }
                GhError::Other(msg) => {
                    inner.consecutive_failures = inner.consecutive_failures.saturating_add(1);
                    warn!(error = %msg, "poll tick errored");
                }
            }
        }
        if auth_changed {
            self.emit_auth_changed().await;
        }
    }

    /// Run a one-shot login probe (used at startup and UI "retry auth").
    pub async fn probe_login(self: &Arc<Self>) -> Option<String> {
        let result = gh_login_status().await;
        let (new_auth, new_login, returned) = match &result {
            Ok(login) => (
                AuthState::Authenticated,
                Some(login.clone()),
                Some(login.clone()),
            ),
            Err(GhError::NotInstalled) => (AuthState::Disabled, None, None),
            Err(_) => (AuthState::NotAuthenticated, None, None),
        };
        let changed = {
            let mut inner = self.inner.lock().await;
            let prev_auth = inner.auth.clone();
            let prev_login = inner.login.clone();
            inner.auth = new_auth.clone();
            inner.login = new_login.clone();
            prev_auth != new_auth || prev_login != new_login
        };
        if changed {
            self.emit_auth_changed().await;
            // If we just came back from Disabled, the run loop is parked on an
            // effectively-infinite sleep. Wake it so polling resumes now
            // instead of never.
            if new_auth != AuthState::Disabled {
                self.force.notify_one();
            }
        }
        returned
    }
}

/// Fetch a single PR's status outside the poll loop. Used by `attach_pr` so
/// the chip shows up immediately (and so attaching a bogus number fails loudly
/// instead of silently sitting there empty until the next tick).
pub async fn fetch_pr_status(
    slug: &GithubSlug,
    number: u32,
) -> Result<Option<GithubPrStatus>, GhError> {
    // The workspace/repo fields only matter for routing poll results back into
    // state, which this path doesn't do.
    let target = Target {
        workspace_id: String::new(),
        repo_key: String::new(),
        slug: slug.clone(),
        kind: TargetKind::Pr(number),
    };
    let targets = std::slice::from_ref(&target);
    let (query, variables) = build_query(targets);
    let data = run_graphql(&query, &variables).await?;
    Ok(parse_response(targets, &data)
        .into_iter()
        .next()
        .and_then(|r| match r.outcome {
            PollOutcome::Status { status, .. } => status,
            // A `Pr` target never yields a discovery.
            PollOutcome::Discovered(_) => None,
        }))
}

/// The distinct values of `items`, order preserved. Discoveries cluster by
/// workspace — several repos of one workspace can each turn up a PR in the same
/// tick — and one refresh per workspace is enough.
fn dedup<'a, I: Iterator<Item = &'a String>>(items: I) -> Vec<&'a String> {
    let mut out: Vec<&String> = Vec::new();
    for item in items {
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

fn backoff_for(failures: u32) -> Duration {
    if failures == 0 {
        return BASE_INTERVAL;
    }
    let secs = BASE_INTERVAL.as_secs() * (1u64 << failures.min(5));
    Duration::from_secs(secs).min(MAX_BACKOFF)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    /// A `PrSource` that returns canned responses and counts calls, so tests
    /// can assert on *whether the loop is alive* without a network or a `gh`
    /// binary.
    #[derive(Default)]
    struct FakeSource {
        calls: Arc<StdMutex<usize>>,
        result: Arc<StdMutex<Option<GhError>>>,
    }

    #[async_trait::async_trait]
    impl PrSource for FakeSource {
        async fn fetch(
            &self,
            _query: &str,
            _variables: &std::collections::BTreeMap<String, String>,
        ) -> Result<Value, GhError> {
            *self.calls.lock().unwrap() += 1;
            match self.result.lock().unwrap().as_ref() {
                Some(GhError::NotInstalled) => Err(GhError::NotInstalled),
                Some(GhError::NotAuthenticated) => Err(GhError::NotAuthenticated),
                Some(e) => Err(GhError::Other(e.to_string())),
                None => Ok(serde_json::json!({})),
            }
        }
    }

    /// A `PrSource` that plays back canned responses in order, so a test can
    /// drive both passes of a tick.
    struct ScriptedSource {
        responses: StdMutex<std::collections::VecDeque<Value>>,
        queries: Arc<StdMutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl PrSource for ScriptedSource {
        async fn fetch(
            &self,
            query: &str,
            _variables: &std::collections::BTreeMap<String, String>,
        ) -> Result<Value, GhError> {
            self.queries.lock().unwrap().push(query.to_string());
            Ok(self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| serde_json::json!({})))
        }
    }

    /// Records the workspaces the store announced as changed.
    struct RecordingNotifier(Arc<StdMutex<Vec<String>>>);

    impl crate::store::WorkspaceNotifier for RecordingNotifier {
        fn workspace_changed(&self, workspace_id: &str) {
            self.0.lock().unwrap().push(workspace_id.to_string());
        }
    }

    #[derive(Default)]
    struct EventLog {
        auth: Arc<StdMutex<Vec<AuthState>>>,
    }

    impl PollerEvents for EventLog {
        fn auth_changed(&self, snapshot: &AuthSnapshot) {
            self.auth.lock().unwrap().push(snapshot.state.clone());
        }
        fn status_changed(&self, _event: &Value) {}
    }

    async fn poller() -> Arc<GithubPoller> {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::load(
            tmp.path().join("state.json"),
            tmp.path().join("state.json.tmp"),
            Box::new(crate::store::NullNotifier),
        )
        .await
        .unwrap();
        // Leak the tempdir: the store's flusher outlives this helper.
        std::mem::forget(tmp);
        Arc::new(GithubPoller::with_seams(
            store,
            Arc::new(RegistryLoad::Missing {
                path: "/nonexistent".into(),
            }),
            Box::new(FakeSource::default()),
            Box::new(EventLog::default()),
        ))
    }

    /// The regression that motivated the seams.
    ///
    /// `gh` missing used to set `Disabled` and `run()` returned — permanently,
    /// since `lib.rs` spawns it exactly once. But `probe_login` would happily
    /// write `Authenticated` back into the same state, so installing `gh`
    /// after boot and hitting retry reported success while polling stayed dead
    /// until the app restarted.
    #[tokio::test]
    async fn recovering_from_disabled_wakes_the_poll_loop() {
        let p = poller().await;

        p.handle_error(GhError::NotInstalled).await;
        assert_eq!(p.auth_snapshot().await.state, AuthState::Disabled);

        // The loop parks rather than returning...
        assert_eq!(p.next_delay().await, Duration::from_secs(u64::MAX));

        // ...so flipping auth back is enough to make it poll again.
        {
            let mut inner = p.inner.lock().await;
            inner.auth = AuthState::Authenticated;
        }
        assert!(
            p.next_delay().await < Duration::from_secs(120),
            "a recovered poller schedules a normal tick"
        );

        // And the wake-up is actually delivered.
        let woken = Arc::new(StdMutex::new(false));
        let w = woken.clone();
        let force = p.force.clone();
        let waiter = tokio::spawn(async move {
            force.notified().await;
            *w.lock().unwrap() = true;
        });
        tokio::task::yield_now().await;
        p.force.notify_one();
        waiter.await.unwrap();
        assert!(*woken.lock().unwrap());
    }

    #[tokio::test]
    async fn auth_state_tracks_the_error_kind() {
        let p = poller().await;

        p.handle_error(GhError::NotAuthenticated).await;
        assert_eq!(p.auth_snapshot().await.state, AuthState::NotAuthenticated);
        assert_eq!(p.next_delay().await, AUTH_RETRY_INTERVAL);

        p.handle_error(GhError::NotInstalled).await;
        assert_eq!(p.auth_snapshot().await.state, AuthState::Disabled);
    }

    /// Network failures back off; a success resets the counter. The pure
    /// `backoff_for` was already tested — this covers the wiring that feeds it.
    #[tokio::test]
    async fn network_failures_accumulate_and_reset() {
        let p = poller().await;

        for _ in 0..3 {
            p.handle_error(GhError::Network("boom".into())).await;
        }
        assert_eq!(p.inner.lock().await.consecutive_failures, 3);
        assert!(p.next_delay().await > BASE_INTERVAL);

        {
            let mut inner = p.inner.lock().await;
            inner.consecutive_failures = 0;
        }
        assert!(p.next_delay().await < BASE_INTERVAL + Duration::from_secs(10));
    }

    /// Window focus/blur storms must not turn into a poll storm.
    #[tokio::test]
    async fn force_ticks_are_rate_limited() {
        let p = poller().await;

        p.request_tick().await;
        let first = p.inner.lock().await.last_forced_tick;
        assert!(first.is_some());

        p.request_tick().await;
        assert_eq!(
            p.inner.lock().await.last_forced_tick,
            first,
            "a second request inside the spacing window is dropped"
        );
    }

    /// A registry that failed to load has no repos, so there is nothing to
    /// poll and we must not issue a request at all.
    #[tokio::test]
    async fn no_registry_means_no_targets() {
        let p = poller().await;
        assert!(p.collect_targets().await.is_empty());
    }


    #[test]
    fn backoff_progression() {
        assert_eq!(backoff_for(0), BASE_INTERVAL);
        assert_eq!(backoff_for(1), Duration::from_secs(90));
        assert_eq!(backoff_for(2), Duration::from_secs(180));
        assert!(backoff_for(10) <= MAX_BACKOFF);
    }

    /// The whole point of the second pass: a PR found by branch scan lands
    /// fully formed, the same as one attached by hand, instead of sitting as a
    /// statusless chip until the next tick 45 seconds later.
    #[tokio::test]
    async fn a_discovered_pr_gets_its_status_in_the_same_tick() {
        let tmp = tempfile::tempdir().unwrap();
        let announced = Arc::new(StdMutex::new(Vec::new()));
        let store = Store::load(
            tmp.path().join("state.json"),
            tmp.path().join("state.json.tmp"),
            Box::new(RecordingNotifier(announced.clone())),
        )
        .await
        .unwrap();
        std::mem::forget(tmp);

        store
            .mutate(|s| {
                let mut ws = crate::state::Workspace::draft(
                    "ws-0".into(),
                    "feat/thing".into(),
                    None,
                    crate::state::Origin::Ui,
                    None,
                );
                ws.repo_links.push(crate::state::RepoLink {
                    repo_key: "api".into(),
                    worktree_path: "/tmp/ws-0/api".into(),
                    setup_script_ran_at: None,
                    prs: Vec::new(),
                    dismissed: Vec::new(),
                    created_branch: true,
                });
                s.workspaces.push(ws);
                Ok(())
            })
            .await
            .unwrap();

        let queries = Arc::new(StdMutex::new(Vec::new()));
        let source = ScriptedSource {
            responses: StdMutex::new(
                [
                    // Pass one: the scan, which only ever yields a number.
                    serde_json::json!({
                        "q0": { "ref": { "associatedPullRequests": { "nodes": [{ "number": 7 }] } } }
                    }),
                    // Pass two: that number's status.
                    serde_json::json!({
                        "q0": { "pullRequest": {
                            "number": 7,
                            "url": "https://github.com/me/api/pull/7",
                            "state": "OPEN",
                            "isDraft": false,
                            "headRefName": "feat/thing",
                            "commits": { "nodes": [{ "commit": {
                                "oid": "sha7",
                                "statusCheckRollup": { "state": "SUCCESS" }
                            }}]}
                        }}
                    }),
                ]
                .into(),
            ),
            queries: queries.clone(),
        };

        let poller = GithubPoller::with_seams(
            store.clone(),
            Arc::new(RegistryLoad::Missing {
                path: "/nonexistent".into(),
            }),
            Box::new(source),
            Box::new(EventLog::default()),
        );

        let targets = vec![Target {
            workspace_id: "ws-0".into(),
            repo_key: "api".into(),
            slug: GithubSlug {
                owner: "me".into(),
                name: "api".into(),
            },
            kind: TargetKind::Branch("feat/thing".into()),
        }];
        poller.do_poll(&targets).await.unwrap();

        assert_eq!(queries.lock().unwrap().len(), 2, "scan, then status fetch");
        // The PR list itself grew, which a `status_changed` can't express — the
        // frontend has to be told to re-read the workspace or the chip never
        // appears.
        assert_eq!(announced.lock().unwrap().as_slice(), ["ws-0"]);
        let status = store
            .read(|s| {
                s.workspaces[0].repo_links[0]
                    .tracked(7)
                    .and_then(|p| p.status.clone())
            })
            .await;
        let status = status.expect("the discovered PR has its status already");
        assert_eq!(status.pr_number, 7);
        assert_eq!(status.checks, crate::github::status::ChecksRollup::Success);
    }
}
