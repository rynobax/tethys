use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde_json::Value;
use tokio::sync::{Mutex, Notify};
use tracing::{info, info_span, trace, warn, Instrument};

use crate::github::client::{gh_login_status, run_graphql, GhCli, GhError, PrSource};
use crate::github::pr_status::{
    apply_results, build_query, parse_response, Applied, PollOutcome, Target, TargetKind,
};
use crate::github::status::GithubPrStatus;
use crate::github::GithubSlug;
use crate::registry::{Registry, RegistryLoad};
use crate::shipping::{self, Shipper};
use crate::state::AppState;
use crate::store::Store;
use crate::ui_socket::Events;

const BASE_INTERVAL: Duration = Duration::from_secs(45);
const MAX_BACKOFF: Duration = Duration::from_secs(600);
const AUTH_RETRY_INTERVAL: Duration = Duration::from_secs(600);
const FORCE_TICK_MIN_SPACING: Duration = Duration::from_secs(10);
const PR_POLL_RETRY: Duration = Duration::from_secs(4);

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
    NotAuthenticated,
    /// `gh` is missing.
    Disabled,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AuthSnapshot {
    pub state: AuthState,
    pub login: Option<String>,
}

pub trait PollerEvents: Send + Sync + 'static {
    fn auth_changed(&self, snapshot: &AuthSnapshot);
    fn status_changed(&self, event: &Value);
    /// A Shipping workspace has a Nudge queued.
    fn nudged(&self, workspace_id: &str);
}

pub struct LiveEvents {
    pub events: Events,
    pub shipper: Arc<Shipper>,
}

impl PollerEvents for LiveEvents {
    fn auth_changed(&self, snapshot: &AuthSnapshot) {
        self.events.emit("github:auth_changed", snapshot);
    }

    fn status_changed(&self, event: &Value) {
        self.events.emit("github:status_changed", event);
    }

    fn nudged(&self, workspace_id: &str) {
        let shipper = self.shipper.clone();
        let workspace_id = workspace_id.to_string();
        tokio::spawn(async move { shipper.wake(&workspace_id).await });
    }
}

pub struct GithubPoller {
    store: Arc<Store>,
    registry: Arc<Registry>,
    source: Box<dyn PrSource>,
    events: Box<dyn PollerEvents>,
    inner: Mutex<PollerInner>,
    force: Arc<Notify>,
}

impl GithubPoller {
    pub fn new(store: Arc<Store>, registry: Arc<Registry>, events: LiveEvents) -> Self {
        Self::with_seams(store, registry, Box::new(GhCli), Box::new(events))
    }

    pub fn with_seams(
        store: Arc<Store>,
        registry: Arc<Registry>,
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

    pub async fn run(self: Arc<Self>) {
        loop {
            let delay = self.next_delay().await;
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = self.force.notified() => {}
            }
            // Park rather than return: `probe_login` wakes us once `gh` is installed.
            if self.inner.lock().await.auth == AuthState::Disabled {
                continue;
            }
            self.tick().await;
        }
    }

    /// Rate-limited so focus/blur churn doesn't become a poll storm.
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

    /// Fetches one PR now, wherever it's tracked, after a write in its PR tab
    /// such as a review request or joining the merge queue.
    pub async fn poll_pr(&self, slug: &GithubSlug, number: u32) {
        let targets: Vec<Target> = self
            .collect_targets()
            .await
            .into_iter()
            .filter(|t| {
                t.kind == TargetKind::Pr(number)
                    && t.slug.owner.eq_ignore_ascii_case(&slug.owner)
                    && t.slug.name.eq_ignore_ascii_case(&slug.name)
            })
            .collect();
        self.poll_until_changed(&targets, PR_POLL_RETRY).await;
    }

    /// A write may not be readable yet when its request returns, so a fetch
    /// that finds nothing new is tried once more.
    async fn poll_until_changed(&self, targets: &[Target], retry_after: Duration) {
        if targets.is_empty() {
            return;
        }
        for attempt in 0..2 {
            if attempt > 0 {
                tokio::time::sleep(retry_after).await;
            }
            match self.fetch_and_apply(targets).await {
                Ok(applied) if !applied.changed.is_empty() => return,
                Ok(_) => {}
                Err(e) => {
                    warn!(error = %e, "PR poll failed");
                    return;
                }
            }
        }
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
        let load = self.registry.get();
        let registry = match load.as_ref() {
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
                out.push(Target {
                    workspace_id: ws.id.clone(),
                    repo_key: link.repo_key.clone(),
                    slug: slug.clone(),
                    kind: TargetKind::Branch(ws.branch.clone()),
                });
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

    /// A PR the branch scan just started tracking is fetched in a second pass,
    /// so it arrives with a status like a hand-attached one.
    async fn do_poll(&self, targets: &[Target]) -> Result<usize, GhError> {
        let applied = self.fetch_and_apply(targets).await?;
        let mut count = applied.changed.len();

        let follow_up: Vec<Target> = applied
            .discovered
            .iter()
            .filter_map(|d| {
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
            let result = self.fetch_and_apply(&follow_up).await;
            // `status_changed` can't add a PR to the list. Announce even if the
            // fetch failed, so the chip still appears.
            for id in dedup(applied.discovered.iter().map(|d| &d.workspace_id)) {
                self.store.notify_changed(id);
            }
            count += result?.changed.len();
        }
        Ok(count)
    }

    async fn fetch_and_apply(&self, targets: &[Target]) -> Result<Applied, GhError> {
        let (query, variables) = build_query(targets);
        trace!(query = %query, variables = ?variables, "github graphql request");
        let asked_at = Utc::now();
        let data = self.source.fetch(&query, &variables).await?;
        trace!(data = %data, "github graphql response");
        let mut parsed = parse_response(targets, &data);
        // Stamped with when it was asked for, so that `apply_results` can drop
        // a slow tick's answer that a later PR poll has already superseded.
        for result in &mut parsed {
            if let PollOutcome::Status {
                status: Some(status),
                ..
            } = &mut result.outcome
            {
                status.fetched_at = asked_at;
            }
        }

        let (applied, shipped) = self
            .store
            .mutate(|s| {
                let applied = apply_results(s, &parsed);
                let reaction = shipping::react(s, &applied.transitions);
                Ok((applied, reaction))
            })
            .await
            .unwrap_or_default();

        for result in &applied.changed {
            if let Some(event) = result.event() {
                self.events.status_changed(&event);
            }
        }
        for id in &shipped.moved {
            self.store.notify_changed(id);
        }
        for id in &shipped.nudged {
            self.events.nudged(id);
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
            // A loop parked by Disabled only resumes if woken.
            if new_auth != AuthState::Disabled {
                self.force.notify_one();
            }
        }
        returned
    }
}

pub async fn fetch_pr_status(
    slug: &GithubSlug,
    number: u32,
) -> Result<Option<GithubPrStatus>, GhError> {
    // Routing fields are unused: nothing is applied to state here.
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
            PollOutcome::Discovered(_) => None,
        }))
}

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

    #[derive(Default)]
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

    #[derive(Default)]
    struct EventLog {
        auth: Arc<StdMutex<Vec<AuthState>>>,
    }

    impl PollerEvents for EventLog {
        fn auth_changed(&self, snapshot: &AuthSnapshot) {
            self.auth.lock().unwrap().push(snapshot.state.clone());
        }
        fn status_changed(&self, _event: &Value) {}
        fn nudged(&self, _workspace_id: &str) {}
    }

    async fn store(notifier: Box<dyn crate::store::WorkspaceNotifier>) -> Arc<Store> {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::load(tmp.path().join("state.json"), notifier)
            .await
            .unwrap();
        // The store's flusher outlives this helper.
        std::mem::forget(tmp);
        store
    }

    fn poller_over(store: Arc<Store>, source: ScriptedSource) -> Arc<GithubPoller> {
        Arc::new(GithubPoller::with_seams(
            store,
            crate::registry::Registry::new(crate::registry::RegistryLoad::Missing {
                path: "/nonexistent".into(),
            }),
            Box::new(source),
            Box::new(EventLog::default()),
        ))
    }

    async fn poller() -> Arc<GithubPoller> {
        poller_over(
            store(Box::new(crate::store::NullNotifier)).await,
            ScriptedSource::default(),
        )
    }

    #[tokio::test]
    async fn recovering_from_disabled_resumes_scheduling() {
        let p = poller().await;

        p.handle_error(GhError::NotInstalled).await;
        assert_eq!(p.auth_snapshot().await.state, AuthState::Disabled);
        assert_eq!(p.next_delay().await, Duration::from_secs(u64::MAX));

        p.inner.lock().await.auth = AuthState::Authenticated;
        assert!(p.next_delay().await < Duration::from_secs(120));
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

    #[tokio::test]
    async fn network_failures_back_off() {
        let p = poller().await;

        for _ in 0..3 {
            p.handle_error(GhError::Network("boom".into())).await;
        }
        assert_eq!(p.inner.lock().await.consecutive_failures, 3);
        assert!(p.next_delay().await > BASE_INTERVAL);
    }

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

    #[tokio::test]
    async fn a_discovered_pr_gets_its_status_in_the_same_tick() {
        let announced = Arc::new(StdMutex::new(Vec::new()));
        let store = store(Box::new(crate::store::RecordingNotifier(announced.clone()))).await;

        store
            .mutate(|s| {
                s.workspaces
                    .push(crate::github::test_support::workspace("ws-0", "feat/thing", "api"));
                Ok(())
            })
            .await
            .unwrap();

        let queries = Arc::new(StdMutex::new(Vec::new()));
        let source = ScriptedSource {
            responses: StdMutex::new(
                [
                    serde_json::json!({
                        "q0": { "ref": { "associatedPullRequests": { "nodes": [{ "number": 7 }] } } }
                    }),
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

        let poller = poller_over(store.clone(), source);

        let targets = vec![Target {
            workspace_id: "ws-0".into(),
            repo_key: "api".into(),
            slug: GithubSlug::new("me", "api"),
            kind: TargetKind::Branch("feat/thing".into()),
        }];
        poller.do_poll(&targets).await.unwrap();

        assert_eq!(queries.lock().unwrap().len(), 2, "scan, then status fetch");
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
    fn pr_response() -> Value {
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
        })
    }

    /// Polls PR 7, tracked with no status yet, against `responses`, and
    /// returns how many fetches it took.
    async fn pr_poll_fetches(responses: Vec<Value>) -> usize {
        let store = store(Box::new(crate::store::NullNotifier)).await;
        store
            .mutate(|s| {
                let mut ws = crate::github::test_support::workspace("ws-0", "feat/thing", "api");
                ws.repo_links[0].track(7, None);
                s.workspaces.push(ws);
                Ok(())
            })
            .await
            .unwrap();
        let queries = Arc::new(StdMutex::new(Vec::new()));
        let source = ScriptedSource {
            responses: StdMutex::new(responses.into()),
            queries: queries.clone(),
        };
        let poller = poller_over(store, source);
        let targets = [Target {
            workspace_id: "ws-0".into(),
            repo_key: "api".into(),
            slug: GithubSlug::new("me", "api"),
            kind: TargetKind::Pr(7),
        }];
        poller.poll_until_changed(&targets, Duration::ZERO).await;
        let n = queries.lock().unwrap().len();
        n
    }

    #[tokio::test]
    async fn a_pr_poll_that_finds_a_change_stops() {
        assert_eq!(pr_poll_fetches(vec![pr_response()]).await, 1);
    }

    #[tokio::test]
    async fn a_pr_poll_that_finds_nothing_new_tries_once_more() {
        let unchanged = serde_json::json!({});
        assert_eq!(pr_poll_fetches(vec![unchanged.clone(), unchanged]).await, 2);
    }
}
