use crate::github::GithubSlug;

/// A user-typed reference to a pull request. `slug` is `None` when the input
/// only carried a number (`123`, `#123`) — the caller then has to decide which
/// repo it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrRef {
    pub slug: Option<GithubSlug>,
    pub number: u32,
}

/// Parse a PR reference the user pasted into the attach dialog. Accepts
/// `123`, `#123`, `owner/repo#123`, `owner/repo/pull/123`, and any
/// `github.com/owner/repo/pull/123` URL (trailing `/files`, `?query`, and
/// `#fragment` are ignored).
pub fn parse_pr_reference(input: &str) -> Option<PrRef> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }

    // Bare number, with or without the `#` sigil.
    if let Some(number) = parse_number(s.strip_prefix('#').unwrap_or(s)) {
        return Some(PrRef { slug: None, number });
    }

    // `owner/repo#123` (and `github.com/owner/repo#123`).
    if let Some((repo_part, num_part)) = s.rsplit_once('#') {
        if let (Some(slug), Some(number)) = (slug_only(repo_part), leading_number(num_part)) {
            return Some(PrRef {
                slug: Some(slug),
                number,
            });
        }
    }

    // Full URL or `owner/repo/pull/123` path.
    let path = strip_host(s)?;
    let mut segments = path.split('/').filter(|p| !p.is_empty());
    let owner = segments.next()?;
    let name = segments.next()?;
    if !matches!(segments.next()?, "pull" | "pulls") {
        return None;
    }
    let number = leading_number(segments.next()?)?;
    Some(PrRef {
        slug: Some(mk_slug(owner, name)?),
        number,
    })
}

/// Parse an `owner/repo` (optionally host-prefixed) reference with nothing
/// after it.
fn slug_only(input: &str) -> Option<GithubSlug> {
    let path = strip_host(input)?;
    let mut segments = path.split('/').filter(|p| !p.is_empty());
    let owner = segments.next()?;
    let name = segments.next()?;
    if segments.next().is_some() {
        return None;
    }
    mk_slug(owner, name)
}

fn mk_slug(owner: &str, name: &str) -> Option<GithubSlug> {
    let name = name.strip_suffix(".git").unwrap_or(name);
    if owner.is_empty() || name.is_empty() {
        return None;
    }
    Some(GithubSlug {
        owner: owner.to_string(),
        name: name.to_string(),
    })
}

/// Drop the scheme and, when present, the host. A host-looking first segment
/// (it contains a dot) must be github.com — a GitLab URL isn't a PR we can
/// poll, so misreading one as `owner/repo` would be worse than rejecting it.
fn strip_host(input: &str) -> Option<&str> {
    let no_scheme = input.split_once("://").map(|(_, r)| r).unwrap_or(input);
    let no_auth = no_scheme
        .split_once('@')
        .filter(|(user, _)| !user.contains('/'))
        .map(|(_, r)| r)
        .unwrap_or(no_scheme);
    match no_auth.split_once('/') {
        Some((first, rest)) if first.contains('.') => {
            let host = first.split_once(':').map(|(h, _)| h).unwrap_or(first);
            host.eq_ignore_ascii_case("github.com").then_some(rest)
        }
        _ => Some(no_auth),
    }
}

/// Digits at the start of `s`, ignoring whatever URL cruft follows.
fn leading_number(s: &str) -> Option<u32> {
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    parse_number(&digits)
}

fn parse_number(s: &str) -> Option<u32> {
    s.parse::<u32>().ok().filter(|n| *n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slug(owner: &str, name: &str) -> Option<GithubSlug> {
        Some(GithubSlug {
            owner: owner.to_string(),
            name: name.to_string(),
        })
    }

    #[test]
    fn parses_bare_number() {
        let r = parse_pr_reference("123").expect("parse");
        assert_eq!(r.number, 123);
        assert_eq!(r.slug, None);
    }

    #[test]
    fn parses_hash_number() {
        let r = parse_pr_reference("  #42 ").expect("parse");
        assert_eq!(r.number, 42);
        assert_eq!(r.slug, None);
    }

    #[test]
    fn parses_owner_repo_hash_number() {
        let r = parse_pr_reference("rynobax/tethys#7").expect("parse");
        assert_eq!(r.number, 7);
        assert_eq!(r.slug, slug("rynobax", "tethys"));
    }

    #[test]
    fn parses_pr_url() {
        let r = parse_pr_reference("https://github.com/rynobax/tethys/pull/99").expect("parse");
        assert_eq!(r.number, 99);
        assert_eq!(r.slug, slug("rynobax", "tethys"));
    }

    #[test]
    fn parses_pr_url_with_trailing_path_and_fragment() {
        let r = parse_pr_reference("https://github.com/rynobax/tethys/pull/99/files#diff-abc")
            .expect("parse");
        assert_eq!(r.number, 99);
        assert_eq!(r.slug, slug("rynobax", "tethys"));
    }

    #[test]
    fn parses_pr_url_with_query_string() {
        let r = parse_pr_reference("https://github.com/rynobax/tethys/pull/12?w=1").expect("parse");
        assert_eq!(r.number, 12);
    }

    #[test]
    fn parses_host_relative_path() {
        let r = parse_pr_reference("rynobax/tethys/pull/3").expect("parse");
        assert_eq!(r.number, 3);
        assert_eq!(r.slug, slug("rynobax", "tethys"));
    }

    #[test]
    fn rejects_non_github_host() {
        assert!(parse_pr_reference("https://gitlab.com/rynobax/tethys/pull/1").is_none());
    }

    #[test]
    fn rejects_issue_url() {
        assert!(parse_pr_reference("https://github.com/rynobax/tethys/issues/4").is_none());
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_pr_reference("").is_none());
        assert!(parse_pr_reference("not a pr").is_none());
        assert!(parse_pr_reference("#0").is_none());
        assert!(parse_pr_reference("#abc").is_none());
    }
}
