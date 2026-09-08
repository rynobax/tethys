//! The embedded GitHub PR viewer.
//!
//! A workspace's side panel grows one tab per linked PR, and each shows the
//! real, logged-in github.com page for that PR. GitHub sends
//! `frame-ancestors 'none'`, so an `<iframe>` — the way a Page artifact
//! renders — draws a blank box no matter who is signed in. The only way to
//! show the live page is a **native child webview**, added to the main window
//! and floated over the panel's body region.
//!
//! It floats *above* the DOM rather than flowing inside it, so the frontend
//! owns the geometry: `SidePanel` measures the panel body's rectangle and
//! calls [`show`] with it on every layout change (panel resize, window
//! resize, tab switch). Switching to Notes, an artifact, or another workspace
//! calls [`hide`].
//!
//! Each PR URL gets its **own** child webview, and switching tabs hides one
//! and shows another rather than renavigating a shared one. A single reused
//! webview was the first cut, and it reloaded the page on every tab switch —
//! a network round trip plus lost scroll position each time — which made the
//! tabs feel like bookmarks rather than open pages. The cost is one WebContent
//! process per live PR, so [`MAX_LIVE`] caps them: the least recently shown is
//! closed when a new one would push past it, and comes back on demand.
//!
//! Login is not inherited from Chrome — a WKWebView can't read Chrome's cookie
//! store. Instead the child webviews use the app's own default (persistent)
//! data store, shared by every webview Tethys owns, so you sign in to GitHub
//! once inside Tethys and it survives restarts.
//!
//! Links: a click that stays within the PR (its tabs, commits, comment
//! anchors) or goes to GitHub's sign-in paths navigates in place; any other
//! link opens in the default browser, as the header chip does. The policy is
//! a page-side click handler ([`link_policy_script`]) that reroutes such
//! clicks to a `tethys-open:` URL, plus a navigation handler that cancels
//! those and runs `open`. Real navigations — redirects, form posts, the SSO
//! round trip — are all allowed, which is what keeps org login working.
//!
//! Trust boundary: `default.json` scopes its permissions to the `"main"`
//! webview, so these `"pr-view-*"` webviews are denied every *plugin* command
//! (`core:*`, `dialog`, `opener`). Application commands, though, aren't part
//! of Tauri's ACL and stay reachable from any webview with an IPC bridge, so
//! the real safeguard is that these webviews are only ever pointed at GitHub —
//! a trusted origin. The one thing the page can ask Tethys for by design is
//! "open this URL in the browser", and only via `tethys-open:`.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

use tauri::webview::{NewWindowResponse, WebviewBuilder};
use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager, Url, WebviewUrl};

use crate::error::{AppError, AppResult};

/// Scheme the page-side click handler navigates to when a link should leave
/// for the browser: `tethys-open:?u=<encoded href>`. The navigation handler
/// cancels it and opens `u`, so a remote page can ask for exactly one thing.
const OPEN_SCHEME: &str = "tethys-open";

/// Fire-and-forget `open <url>`: the default browser, same as the header chip.
fn open_in_browser(url: &str) {
    if let Err(e) = std::process::Command::new("open").arg(url).spawn() {
        tracing::warn!(url, %e, "failed to open in browser");
    }
}

/// The script every page in a PR webview runs: a capturing click handler that
/// keeps links *within this PR* (and GitHub's own sign-in paths) inside the
/// panel and sends every other link to the default browser.
///
/// It has to be a click handler rather than only the navigation handler
/// because GitHub is a Turbo app: many links are fetched over XHR and applied
/// with `pushState`, which never reaches `decidePolicyForNavigationAction`.
/// Catching the click is the only way to see them. The handler only acts on
/// pages from the PR's own host, so an SSO provider's pages are left alone
/// mid-login, and it never touches non-`http(s)` links.
fn link_policy_script(pr: &Url) -> String {
    let host = serde_json::to_string(pr.host_str().unwrap_or_default())
        .unwrap_or_else(|_| "\"\"".into());
    let path = serde_json::to_string(pr.path()).unwrap_or_else(|_| "\"\"".into());
    format!(
        r#"(() => {{
  const PR_HOST = {host};
  const PR_PATH = {path};
  const STAY = [PR_PATH, "/login", "/session", "/sessions", "/sso", "/saml",
                "/oauth", "/password_reset", "/settings/two_factor"];
  const under = (p, base) => p === base || p.startsWith(base + "/");
  const stays = (u) =>
    u.host === PR_HOST &&
    (STAY.some((base) => under(u.pathname, base)) ||
      (u.pathname.startsWith("/orgs/") && u.pathname.includes("/sso")));
  document.addEventListener("click", (e) => {{
    if (e.defaultPrevented || e.button !== 0) return;
    if (location.host !== PR_HOST) return;
    const a = e.target instanceof Element ? e.target.closest("a[href]") : null;
    if (!a) return;
    let u;
    try {{ u = new URL(a.href, location.href); }} catch {{ return; }}
    if (u.protocol !== "http:" && u.protocol !== "https:") return;
    if (stays(u)) return;
    e.preventDefault();
    e.stopImmediatePropagation();
    location.href = "{OPEN_SCHEME}:?u=" + encodeURIComponent(u.href);
  }}, true);
}})();"#
    )
}

/// Label prefix for every PR webview. `default.json` deliberately lists none
/// of them, so they hold no permissions.
const LABEL_PREFIX: &str = "pr-view-";

/// How many PR pages stay alive at once, across every workspace. Each is a
/// WebContent process holding a rendered GitHub page — tens of megabytes on a
/// machine that is already short of them — so this is a memory cap, not a
/// feature: a PR past it just reloads the next time its tab is picked.
const MAX_LIVE: usize = 6;

/// Labels of the live PR webviews, least recently shown first.
static LIVE: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn label_for(url: &Url) -> String {
    let mut hasher = DefaultHasher::new();
    url.as_str().hash(&mut hasher);
    format!("{LABEL_PREFIX}{:016x}", hasher.finish())
}

/// Where the main webview's top-left sits inside the window's content view,
/// in logical pixels. The frontend measures in its own viewport, but a child
/// webview is placed in the content view, and the two origins aren't the same:
/// Tauri's default title bar style gives the window a full-size content view
/// that runs up under the (opaque) title bar, and the main webview sits below
/// it. Placing a child at raw DOM coordinates put it a title bar too high —
/// over the tab's toolbar, with an empty strip left at the bottom. Read from
/// the live webview rather than hard-coded, so a title bar style change or a
/// Tauri fix leaves this correct.
fn main_origin(app: &AppHandle) -> AppResult<(f64, f64)> {
    let Some(main) = app.get_webview("main") else {
        return Ok((0.0, 0.0));
    };
    let scale = main.window().scale_factor()?;
    let position = main.bounds()?.position.to_logical::<f64>(scale);
    Ok((position.x, position.y))
}

/// Position and show the webview for `url` over the given rectangle, hiding
/// every other PR webview. Coordinates are logical pixels in the main window's
/// content area — the same space `getBoundingClientRect` reports in, since the
/// main webview fills that area from its origin.
///
/// Creates the webview on first sight of the URL and reuses it after, so a
/// resize, a re-show of the tab you were on, or a return to a tab you left all
/// keep the page — and its scroll position — exactly as it was.
pub fn show(
    app: &AppHandle,
    url: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> AppResult<()> {
    let target =
        Url::parse(&url).map_err(|e| AppError::Other(format!("bad PR url {url:?}: {e}")))?;
    let label = label_for(&target);
    let (dx, dy) = main_origin(app)?;
    tracing::debug!(dx, dy, x, y, width, height, "placing PR webview");
    let position = LogicalPosition::new(x + dx, y + dy);
    let size = LogicalSize::new(width, height);

    let mut live = LIVE.lock().unwrap_or_else(|p| p.into_inner());

    for other in live.iter().filter(|l| **l != label) {
        if let Some(webview) = app.get_webview(other) {
            webview.hide()?;
        }
    }

    // Move (or insert) this label at the most-recent end.
    live.retain(|l| *l != label);
    live.push(label.clone());

    if let Some(webview) = app.get_webview(&label) {
        webview.set_position(position)?;
        webview.set_size(size)?;
        webview.show()?;
        return Ok(());
    }

    // The main `WebviewWindow` is also registered as a plain `Window`, which is
    // the handle that hosts additional child webviews.
    let window = app
        .get_window("main")
        .ok_or_else(|| AppError::Other("main window is not open".into()))?;
    let builder = WebviewBuilder::new(&label, WebviewUrl::External(target.clone()))
        .initialization_script(link_policy_script(&target))
        .on_navigation(|url| {
            if url.scheme() != OPEN_SCHEME {
                return true;
            }
            match url.query_pairs().find(|(k, _)| k == "u") {
                Some((_, href)) => open_in_browser(&href),
                None => tracing::warn!(%url, "open request without a url"),
            }
            false
        })
        .on_new_window(|url, _| {
            open_in_browser(url.as_str());
            NewWindowResponse::Deny
        });
    window.add_child(builder, position, size)?;

    // Evict the least recently shown past the cap. Closing is best-effort: a
    // webview that already went away just drops out of the list.
    while live.len() > MAX_LIVE {
        let oldest = live.remove(0);
        if let Some(webview) = app.get_webview(&oldest) {
            webview.close()?;
        }
    }
    Ok(())
}

/// Hide every PR webview. Called whenever the visible tab stops being a PR —
/// Notes, an artifact, a collapsed panel, or a switch to a workspace with no
/// PR tab open. Hiding rather than closing keeps each page warm for the next
/// time its tab is selected.
pub fn hide(app: &AppHandle) -> AppResult<()> {
    let live = LIVE.lock().unwrap_or_else(|p| p.into_inner());
    for label in live.iter() {
        if let Some(webview) = app.get_webview(label) {
            webview.hide()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The page sends `tethys-open:?u=<encodeURIComponent(href)>`; the
    /// navigation handler has to get the exact href back out. A scheme with no
    /// authority is a "cannot-be-a-base" URL, and this pins down that `url`
    /// still parses its query for us.
    #[test]
    fn open_request_round_trips_the_href() {
        let href = "https://github.com/new-lantern/nl-ai/pull/536/files#diff-abc";
        let encoded = "https%3A%2F%2Fgithub.com%2Fnew-lantern%2Fnl-ai%2Fpull%2F536%2Ffiles%23diff-abc";
        let url = Url::parse(&format!("{OPEN_SCHEME}:?u={encoded}")).unwrap();
        assert_eq!(url.scheme(), OPEN_SCHEME);
        let (_, got) = url.query_pairs().find(|(k, _)| k == "u").unwrap();
        assert_eq!(got, href);
    }

    #[test]
    fn link_policy_script_embeds_the_pr_as_json_strings() {
        let pr = Url::parse("https://github.com/new-lantern/nl-ai/pull/536").unwrap();
        let script = link_policy_script(&pr);
        assert!(script.contains(r#"const PR_HOST = "github.com";"#));
        assert!(script.contains(r#"const PR_PATH = "/new-lantern/nl-ai/pull/536";"#));
        assert!(script.contains(&format!(r#""{OPEN_SCHEME}:?u=""#)));
    }
}
