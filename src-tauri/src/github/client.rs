use std::collections::BTreeMap;
use std::io;
use std::process::Stdio;

use serde_json::Value;
use thiserror::Error;
use tokio::process::Command;

#[derive(Debug, Error)]
pub enum GhError {
    #[error("gh CLI is not installed or not on PATH")]
    NotInstalled,

    #[error("gh is not authenticated (run `gh auth login`)")]
    NotAuthenticated,

    #[error("GitHub rate limit exceeded")]
    RateLimited,

    #[error("network error talking to GitHub: {0}")]
    Network(String),

    /// One or more GraphQL-level errors came back. `data` may still be partially populated.
    #[error("GraphQL errors: {0:?}")]
    Graphql(Vec<String>),

    #[error("gh exited with an error: {0}")]
    Other(String),
}

impl GhError {
    fn from_io(err: io::Error) -> Self {
        if err.kind() == io::ErrorKind::NotFound {
            GhError::NotInstalled
        } else {
            GhError::Other(err.to_string())
        }
    }
}

/// Where PR status comes from.
///
/// The seam between "talk to `gh`" and "decide what to poll and when". One
/// method, because that's all the poller needs; two adapters, because tests
/// need a source that doesn't shell out to a binary the CI box may not have
/// and doesn't hit the network.
#[async_trait::async_trait]
pub trait PrSource: Send + Sync + 'static {
    async fn fetch(
        &self,
        query: &str,
        variables: &BTreeMap<String, String>,
    ) -> Result<Value, GhError>;
}

/// Production adapter: the `gh` CLI.
pub struct GhCli;

#[async_trait::async_trait]
impl PrSource for GhCli {
    async fn fetch(
        &self,
        query: &str,
        variables: &BTreeMap<String, String>,
    ) -> Result<Value, GhError> {
        run_graphql(query, variables).await
    }
}

/// Run a GraphQL query via `gh api graphql` and return the parsed `data` payload.
///
/// `variables` is a map of string-valued GraphQL variables. The query we send
/// only references string variables (owner, name, branch qualifier), so this
/// intentionally does not support nested JSON.
pub async fn run_graphql(
    query: &str,
    variables: &BTreeMap<String, String>,
) -> Result<Value, GhError> {
    let mut cmd = Command::new("gh");
    cmd.args(["api", "graphql", "-f", &format!("query={query}")]);
    for (key, value) in variables {
        cmd.arg("-f").arg(format!("{key}={value}"));
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = cmd.output().await.map_err(GhError::from_io)?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        return Err(classify_failure(&stdout, &stderr));
    }

    let parsed: Value = serde_json::from_str(&stdout)
        .map_err(|e| GhError::Other(format!("invalid JSON from gh: {e}")))?;

    if let Some(errors) = parsed.get("errors").and_then(|e| e.as_array()) {
        let messages: Vec<String> = errors
            .iter()
            .map(|e| {
                e.get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("(unknown)")
                    .to_string()
            })
            .collect();
        return Err(GhError::Graphql(messages));
    }

    parsed
        .get("data")
        .cloned()
        .ok_or_else(|| GhError::Other("gh response missing `data` field".to_string()))
}

/// Ask `gh` who the authenticated user is. Returns the login handle on success.
pub async fn gh_login_status() -> Result<String, GhError> {
    let output = Command::new("gh")
        .args(["api", "user", "--jq", ".login"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(GhError::from_io)?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        return Err(classify_failure(&stdout, &stderr));
    }

    let login = stdout.trim();
    if login.is_empty() {
        return Err(GhError::Other("gh returned empty login".to_string()));
    }
    Ok(login.to_string())
}

fn classify_failure(stdout: &str, stderr: &str) -> GhError {
    let haystack = format!("{stdout}\n{stderr}").to_lowercase();
    if haystack.contains("not logged")
        || haystack.contains("authentication required")
        || haystack.contains("gh auth login")
    {
        return GhError::NotAuthenticated;
    }
    if haystack.contains("rate limit") || haystack.contains("api rate limit exceeded") {
        return GhError::RateLimited;
    }
    if haystack.contains("could not resolve host")
        || haystack.contains("connection refused")
        || haystack.contains("network is unreachable")
        || haystack.contains("dial tcp")
    {
        return GhError::Network(stderr.trim().to_string());
    }
    GhError::Other(format!("stderr: {}", stderr.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_not_authenticated() {
        let err = classify_failure("", "error: you are not logged in");
        assert!(matches!(err, GhError::NotAuthenticated));
    }

    #[test]
    fn classify_rate_limit() {
        let err = classify_failure("", "API rate limit exceeded for user");
        assert!(matches!(err, GhError::RateLimited));
    }

    #[test]
    fn classify_network() {
        let err = classify_failure("", "dial tcp: lookup api.github.com: no such host");
        assert!(matches!(err, GhError::Network(_)));
    }

    #[test]
    fn classify_other_falls_through() {
        let err = classify_failure("", "something weird happened");
        assert!(matches!(err, GhError::Other(_)));
    }
}
