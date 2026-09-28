use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GithubSlug {
    pub owner: String,
    pub name: String,
}

impl GithubSlug {
    pub fn new(owner: &str, name: &str) -> Self {
        GithubSlug {
            owner: owner.to_string(),
            name: name.to_string(),
        }
    }
}

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

    Some(GithubSlug::new(owner, name))
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

    #[test]
    fn ssh_scp_form_with_dot_git() {
        assert_eq!(
            parse_github_remote("git@github.com:ryan/tethys.git"),
            Some(GithubSlug::new("ryan", "tethys"))
        );
    }

    #[test]
    fn ssh_scp_form_without_dot_git() {
        assert_eq!(
            parse_github_remote("git@github.com:ryan/tethys"),
            Some(GithubSlug::new("ryan", "tethys"))
        );
    }

    #[test]
    fn https_form_with_dot_git() {
        assert_eq!(
            parse_github_remote("https://github.com/ryan/tethys.git"),
            Some(GithubSlug::new("ryan", "tethys"))
        );
    }

    #[test]
    fn https_form_without_dot_git() {
        assert_eq!(
            parse_github_remote("https://github.com/ryan/tethys"),
            Some(GithubSlug::new("ryan", "tethys"))
        );
    }

    #[test]
    fn https_form_with_trailing_slash() {
        assert_eq!(
            parse_github_remote("https://github.com/ryan/tethys/"),
            Some(GithubSlug::new("ryan", "tethys"))
        );
    }

    #[test]
    fn https_form_with_userinfo() {
        assert_eq!(
            parse_github_remote("https://token@github.com/ryan/tethys.git"),
            Some(GithubSlug::new("ryan", "tethys"))
        );
    }

    #[test]
    fn ssh_url_form() {
        assert_eq!(
            parse_github_remote("ssh://git@github.com/ryan/tethys.git"),
            Some(GithubSlug::new("ryan", "tethys"))
        );
    }

    #[test]
    fn git_protocol_form() {
        assert_eq!(
            parse_github_remote("git://github.com/ryan/tethys.git"),
            Some(GithubSlug::new("ryan", "tethys"))
        );
    }

    #[test]
    fn host_is_case_insensitive() {
        assert_eq!(
            parse_github_remote("https://GitHub.com/ryan/tethys"),
            Some(GithubSlug::new("ryan", "tethys"))
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
        assert_eq!(
            parse_github_remote("https://github.com/ryan/tethys/tree/main"),
            None
        );
    }

    #[test]
    fn whitespace_trimmed() {
        assert_eq!(
            parse_github_remote("  git@github.com:ryan/tethys.git  "),
            Some(GithubSlug::new("ryan", "tethys"))
        );
    }
}
