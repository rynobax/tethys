use crate::github::GithubSlug;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrRef {
    pub slug: Option<GithubSlug>,
    pub number: u32,
}

/// Accepts `123`, `#123`, `owner/repo#123`, `owner/repo/pull/123`, and PR URLs.
pub fn parse_pr_reference(input: &str) -> Option<PrRef> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }

    if let Some(number) = parse_number(s.strip_prefix('#').unwrap_or(s)) {
        return Some(PrRef { slug: None, number });
    }

    if let Some((repo_part, num_part)) = s.rsplit_once('#') {
        if let (Some(slug), Some(number)) = (slug_only(repo_part), leading_number(num_part)) {
            return Some(PrRef {
                slug: Some(slug),
                number,
            });
        }
    }

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
    Some(GithubSlug::new(owner, name))
}

/// A dotted first segment is a host, and must be github.com rather than
/// misread as an owner.
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

fn leading_number(s: &str) -> Option<u32> {
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    parse_number(&digits)
}

fn parse_number(s: &str) -> Option<u32> {
    s.parse::<u32>().ok().filter(|n| *n > 0)
}

pub fn resolve_attach_target(
    candidates: &[(String, GithubSlug)],
    explicit_repo_key: Option<&str>,
    parsed: &PrRef,
) -> Result<(String, GithubSlug), AttachError> {
    let (repo_key, slug) = match (explicit_repo_key, &parsed.slug) {
        (Some(key), _) => candidates
            .iter()
            .find(|(k, _)| k == key)
            .cloned()
            .ok_or_else(|| AttachError::NotAGithubRepo {
                repo_key: key.to_string(),
            })?,
        (None, Some(want)) => candidates
            .iter()
            .find(|(_, slug)| slug == want)
            .cloned()
            .ok_or_else(|| AttachError::NoRepoForSlug {
                slug: want.clone(),
            })?,
        (None, None) if candidates.len() == 1 => candidates[0].clone(),
        (None, None) if candidates.is_empty() => return Err(AttachError::NoGithubRepos),
        (None, None) => return Err(AttachError::Ambiguous),
    };

    if let Some(want) = &parsed.slug {
        if want != &slug {
            return Err(AttachError::SlugMismatch {
                number: parsed.number,
                pasted: want.clone(),
                repo_key,
                configured: slug,
            });
        }
    }
    Ok((repo_key, slug))
}

#[derive(Debug, PartialEq, Eq)]
pub enum AttachError {
    NotAGithubRepo { repo_key: String },
    NoRepoForSlug { slug: GithubSlug },
    NoGithubRepos,
    Ambiguous,
    SlugMismatch {
        number: u32,
        pasted: GithubSlug,
        repo_key: String,
        configured: GithubSlug,
    },
}

impl std::fmt::Display for AttachError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AttachError::NotAGithubRepo { repo_key } => {
                write!(f, "{repo_key} isn't a GitHub-linked repo in this workspace")
            }
            AttachError::NoRepoForSlug { slug } => write!(
                f,
                "no repo in this workspace points at {}/{}",
                slug.owner, slug.name
            ),
            AttachError::NoGithubRepos => {
                write!(f, "this workspace has no GitHub-linked repos to attach a PR to")
            }
            AttachError::Ambiguous => write!(
                f,
                "this workspace has more than one GitHub repo — pick which one the PR belongs to"
            ),
            AttachError::SlugMismatch {
                number,
                pasted,
                repo_key,
                configured,
            } => write!(
                f,
                "PR #{number} is in {}/{}, but repo {repo_key} points at {}/{}",
                pasted.owner, pasted.name, configured.owner, configured.name
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidates(pairs: &[(&str, &str, &str)]) -> Vec<(String, GithubSlug)> {
        pairs
            .iter()
            .map(|(key, owner, name)| (key.to_string(), GithubSlug::new(owner, name)))
            .collect()
    }

    fn bare(number: u32) -> PrRef {
        PrRef { slug: None, number }
    }

    fn qualified(owner: &str, name: &str, number: u32) -> PrRef {
        PrRef {
            slug: Some(GithubSlug::new(owner, name)),
            number,
        }
    }

    #[test]
    fn a_bare_number_resolves_when_there_is_only_one_repo() {
        let c = candidates(&[("api", "me", "api")]);
        let (key, s) = resolve_attach_target(&c, None, &bare(12)).unwrap();
        assert_eq!(key, "api");
        assert_eq!(s, GithubSlug::new("me", "api"));
    }

    #[test]
    fn a_bare_number_is_ambiguous_with_two_repos() {
        let c = candidates(&[("api", "me", "api"), ("web", "me", "web")]);
        assert_eq!(
            resolve_attach_target(&c, None, &bare(12)),
            Err(AttachError::Ambiguous)
        );
    }

    #[test]
    fn a_bare_number_with_no_github_repos_says_so() {
        assert_eq!(
            resolve_attach_target(&[], None, &bare(12)),
            Err(AttachError::NoGithubRepos)
        );
    }

    #[test]
    fn a_pasted_url_picks_its_own_repo_out_of_several() {
        let c = candidates(&[("api", "me", "api"), ("web", "me", "web")]);
        let (key, _) = resolve_attach_target(&c, None, &qualified("me", "web", 3)).unwrap();
        assert_eq!(key, "web");
    }

    #[test]
    fn a_pasted_url_for_a_repo_outside_the_workspace_is_rejected() {
        let c = candidates(&[("api", "me", "api")]);
        assert_eq!(
            resolve_attach_target(&c, None, &qualified("other", "thing", 3)),
            Err(AttachError::NoRepoForSlug {
                slug: GithubSlug::new("other", "thing")
            })
        );
    }

    #[test]
    fn an_explicit_repo_key_wins_for_a_bare_number() {
        let c = candidates(&[("api", "me", "api"), ("web", "me", "web")]);
        let (key, _) = resolve_attach_target(&c, Some("web"), &bare(9)).unwrap();
        assert_eq!(key, "web");
    }

    #[test]
    fn an_explicit_repo_key_that_is_not_github_linked_is_rejected() {
        let c = candidates(&[("api", "me", "api")]);
        assert_eq!(
            resolve_attach_target(&c, Some("docs"), &bare(9)),
            Err(AttachError::NotAGithubRepo {
                repo_key: "docs".into()
            })
        );
    }

    #[test]
    fn an_explicit_repo_disagreeing_with_the_pasted_url_is_reported() {
        let c = candidates(&[("api", "me", "api"), ("web", "me", "web")]);
        let err = resolve_attach_target(&c, Some("api"), &qualified("me", "web", 42)).unwrap_err();
        assert_eq!(
            err,
            AttachError::SlugMismatch {
                number: 42,
                pasted: GithubSlug::new("me", "web"),
                repo_key: "api".into(),
                configured: GithubSlug::new("me", "api"),
            }
        );
        let msg = err.to_string();
        assert!(msg.contains("#42") && msg.contains("me/web") && msg.contains("me/api"), "{msg}");
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
        assert_eq!(r.slug, Some(GithubSlug::new("rynobax", "tethys")));
    }

    #[test]
    fn parses_pr_url() {
        let r = parse_pr_reference("https://github.com/rynobax/tethys/pull/99").expect("parse");
        assert_eq!(r.number, 99);
        assert_eq!(r.slug, Some(GithubSlug::new("rynobax", "tethys")));
    }

    #[test]
    fn parses_pr_url_with_trailing_path_and_fragment() {
        let r = parse_pr_reference("https://github.com/rynobax/tethys/pull/99/files#diff-abc")
            .expect("parse");
        assert_eq!(r.number, 99);
        assert_eq!(r.slug, Some(GithubSlug::new("rynobax", "tethys")));
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
        assert_eq!(r.slug, Some(GithubSlug::new("rynobax", "tethys")));
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
