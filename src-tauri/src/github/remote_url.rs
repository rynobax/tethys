use serde::Serialize;

/// A parsed `owner/name` identifier for a GitHub repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GithubSlug {
    pub owner: String,
    pub name: String,
}

/// Parse a git remote URL into a GitHub `(owner, name)` slug.
///
/// Returns `None` for non-GitHub hosts or malformed input.
pub fn parse_github_remote(url: &str) -> Option<GithubSlug> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }

    let (host, path) = split_host_and_path(trimmed)?;
    if !is_github_host(host) {
        return None;
    }

    let mut segments = path.trim_start_matches('/').trim_end_matches('/').splitn(2, '/');
    let owner = segments.next()?.trim();
    let name_raw = segments.next()?.trim();
    if owner.is_empty() || name_raw.is_empty() {
        return None;
    }

    let name = name_raw.strip_suffix(".git").unwrap_or(name_raw);
    if name.is_empty() || name.contains('/') {
        return None;
    }

    Some(GithubSlug {
        owner: owner.to_string(),
        name: name.to_string(),
    })
}

/// Parse a GitHub pull-request URL into its repo slug and PR number.
///
/// Accepts the canonical web form `https://github.com/owner/name/pull/123`
/// (with or without a trailing path like `/files`, and with or without a
/// `?query`/`#fragment`). Returns `None` for non-GitHub hosts, non-PR paths,
/// or malformed input.
pub fn parse_github_pr_url(url: &str) -> Option<(GithubSlug, u32)> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }

    let (host, path) = split_host_and_path(trimmed)?;
    if !is_github_host(host) {
        return None;
    }

    // Drop any query string / fragment before splitting path segments.
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let mut segments = path.trim_start_matches('/').split('/');
    let owner = segments.next()?.trim();
    let name_raw = segments.next()?.trim();
    // GitHub uses `/pull/` for the web UI; `/pulls/` is the list page, not a PR.
    if segments.next()? != "pull" {
        return None;
    }
    let number: u32 = segments.next()?.trim().parse().ok()?;
    if owner.is_empty() || name_raw.is_empty() {
        return None;
    }
    let name = name_raw.strip_suffix(".git").unwrap_or(name_raw);
    if name.is_empty() {
        return None;
    }

    Some((
        GithubSlug {
            owner: owner.to_string(),
            name: name.to_string(),
        },
        number,
    ))
}

fn is_github_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("github.com")
}

fn split_host_and_path(url: &str) -> Option<(&str, &str)> {
    if let Some(rest) = url.strip_prefix("git@") {
        let (host, path) = rest.split_once(':')?;
        return Some((host, path));
    }
    if let Some(rest) = url.strip_prefix("ssh://") {
        let after_auth = rest.split_once('@').map(|(_, r)| r).unwrap_or(rest);
        let (host, path) = after_auth.split_once('/')?;
        return Some((host, path));
    }
    for prefix in ["https://", "http://", "git://"] {
        if let Some(rest) = url.strip_prefix(prefix) {
            let after_auth = rest.split_once('@').map(|(_, r)| r).unwrap_or(rest);
            let (host, path) = after_auth.split_once('/')?;
            return Some((host, path));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slug(owner: &str, name: &str) -> GithubSlug {
        GithubSlug {
            owner: owner.to_string(),
            name: name.to_string(),
        }
    }

    #[test]
    fn ssh_scp_form_with_dot_git() {
        assert_eq!(
            parse_github_remote("git@github.com:ryan/tethys.git"),
            Some(slug("ryan", "tethys"))
        );
    }

    #[test]
    fn ssh_scp_form_without_dot_git() {
        assert_eq!(
            parse_github_remote("git@github.com:ryan/tethys"),
            Some(slug("ryan", "tethys"))
        );
    }

    #[test]
    fn https_form_with_dot_git() {
        assert_eq!(
            parse_github_remote("https://github.com/ryan/tethys.git"),
            Some(slug("ryan", "tethys"))
        );
    }

    #[test]
    fn https_form_without_dot_git() {
        assert_eq!(
            parse_github_remote("https://github.com/ryan/tethys"),
            Some(slug("ryan", "tethys"))
        );
    }

    #[test]
    fn https_form_with_trailing_slash() {
        assert_eq!(
            parse_github_remote("https://github.com/ryan/tethys/"),
            Some(slug("ryan", "tethys"))
        );
    }

    #[test]
    fn https_form_with_userinfo() {
        assert_eq!(
            parse_github_remote("https://token@github.com/ryan/tethys.git"),
            Some(slug("ryan", "tethys"))
        );
    }

    #[test]
    fn ssh_url_form() {
        assert_eq!(
            parse_github_remote("ssh://git@github.com/ryan/tethys.git"),
            Some(slug("ryan", "tethys"))
        );
    }

    #[test]
    fn git_protocol_form() {
        assert_eq!(
            parse_github_remote("git://github.com/ryan/tethys.git"),
            Some(slug("ryan", "tethys"))
        );
    }

    #[test]
    fn host_is_case_insensitive() {
        assert_eq!(
            parse_github_remote("https://GitHub.com/ryan/tethys"),
            Some(slug("ryan", "tethys"))
        );
    }

    #[test]
    fn gitlab_returns_none() {
        assert_eq!(parse_github_remote("git@gitlab.com:ryan/tethys.git"), None);
        assert_eq!(parse_github_remote("https://gitlab.com/ryan/tethys.git"), None);
    }

    #[test]
    fn bitbucket_returns_none() {
        assert_eq!(parse_github_remote("https://bitbucket.org/ryan/tethys.git"), None);
    }

    #[test]
    fn github_enterprise_returns_none() {
        // Enterprise hosts aren't supported — `gh` handles GH_HOST, but we
        // only tag the primary github.com here. Revisit if we add enterprise support.
        assert_eq!(parse_github_remote("git@github.mycorp.com:ryan/tethys.git"), None);
    }

    #[test]
    fn empty_and_garbage_return_none() {
        assert_eq!(parse_github_remote(""), None);
        assert_eq!(parse_github_remote("   "), None);
        assert_eq!(parse_github_remote("not-a-url"), None);
        assert_eq!(parse_github_remote("github.com/ryan/tethys"), None);
    }

    #[test]
    fn missing_name_returns_none() {
        assert_eq!(parse_github_remote("git@github.com:ryan/"), None);
        assert_eq!(parse_github_remote("https://github.com/ryan"), None);
        assert_eq!(parse_github_remote("https://github.com/ryan/"), None);
    }

    #[test]
    fn missing_owner_returns_none() {
        assert_eq!(parse_github_remote("git@github.com:/tethys.git"), None);
        assert_eq!(parse_github_remote("https://github.com//tethys"), None);
    }

    #[test]
    fn extra_path_segments_rejected() {
        // owner/name is the entire path; deeper paths (e.g. /tree/main)
        // are ambiguous and we don't want to silently strip.
        assert_eq!(
            parse_github_remote("https://github.com/ryan/tethys/tree/main"),
            None
        );
    }

    #[test]
    fn whitespace_trimmed() {
        assert_eq!(
            parse_github_remote("  git@github.com:ryan/tethys.git  "),
            Some(slug("ryan", "tethys"))
        );
    }

    #[test]
    fn pr_url_canonical() {
        assert_eq!(
            parse_github_pr_url("https://github.com/ryan/tethys/pull/42"),
            Some((slug("ryan", "tethys"), 42))
        );
    }

    #[test]
    fn pr_url_with_trailing_path() {
        assert_eq!(
            parse_github_pr_url("https://github.com/ryan/tethys/pull/42/files"),
            Some((slug("ryan", "tethys"), 42))
        );
    }

    #[test]
    fn pr_url_with_query_and_fragment() {
        assert_eq!(
            parse_github_pr_url("https://github.com/ryan/tethys/pull/42?diff=split#discussion_r1"),
            Some((slug("ryan", "tethys"), 42))
        );
    }

    #[test]
    fn pr_url_host_case_insensitive_and_trimmed() {
        assert_eq!(
            parse_github_pr_url("  https://GitHub.com/ryan/tethys/pull/7  "),
            Some((slug("ryan", "tethys"), 7))
        );
    }

    #[test]
    fn pr_url_rejects_non_pr_paths() {
        assert_eq!(parse_github_pr_url("https://github.com/ryan/tethys"), None);
        assert_eq!(
            parse_github_pr_url("https://github.com/ryan/tethys/issues/42"),
            None
        );
        assert_eq!(
            parse_github_pr_url("https://github.com/ryan/tethys/pulls"),
            None
        );
    }

    #[test]
    fn pr_url_rejects_non_numeric_number() {
        assert_eq!(
            parse_github_pr_url("https://github.com/ryan/tethys/pull/abc"),
            None
        );
        assert_eq!(
            parse_github_pr_url("https://github.com/ryan/tethys/pull/"),
            None
        );
    }

    #[test]
    fn pr_url_rejects_non_github_host() {
        assert_eq!(
            parse_github_pr_url("https://gitlab.com/ryan/tethys/pull/42"),
            None
        );
    }

    #[test]
    fn pr_url_empty_and_garbage() {
        assert_eq!(parse_github_pr_url(""), None);
        assert_eq!(parse_github_pr_url("   "), None);
        assert_eq!(parse_github_pr_url("not-a-url"), None);
    }
}
