//! GitHub sends `frame-ancestors 'none'`, so a live PR page can't be an
//! iframe: it's a native child webview floated over the panel, placed at a
//! rect the frontend measures. One webview per PR URL, hidden rather than
//! renavigated, so page and scroll survive a tab switch.
//!
//! Trust boundary: these webviews hold no plugin permissions, but app commands
//! aren't ACL-gated, so the real safeguard is only ever loading GitHub. The one
//! thing a page can ask Tethys for is `tethys-open:`.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

use tauri::webview::{NewWindowResponse, WebviewBuilder};
use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager, Url, WebviewUrl};

use crate::error::{AppError, AppResult};

/// `tethys-open:?u=<encoded href>`, cancelled by the navigation handler and
/// opened in the browser.
const OPEN_SCHEME: &str = "tethys-open";

fn open_in_browser(url: &str) {
    if let Err(e) = std::process::Command::new("open").arg(url).spawn() {
        tracing::warn!(url, %e, "failed to open in browser");
    }
}

/// Keeps links within the PR and GitHub's sign-in paths in the panel and sends
/// the rest to the browser. A click handler because GitHub's Turbo links use
/// `pushState` and never reach the navigation handler; it ignores other hosts
/// so an SSO provider's pages are left alone mid-login.
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

const LABEL_PREFIX: &str = "pr-view-";

/// Across all workspaces. Each is a WebContent process of tens of megabytes.
const MAX_LIVE: usize = 6;

/// Labels of the live PR webviews, least recently shown first.
static LIVE: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn label_for(url: &Url) -> String {
    let mut hasher = DefaultHasher::new();
    url.as_str().hash(&mut hasher);
    format!("{LABEL_PREFIX}{:016x}", hasher.finish())
}

/// WebKit insets the main page below the title bar without any frame
/// reporting it, so DOM `y = 0` isn't the content view's. The inset shows up
/// only as the gap between the webview's height and the page's `innerHeight`.
fn main_origin(app: &AppHandle, viewport_height: f64) -> AppResult<(f64, f64)> {
    let Some(main) = app.get_webview("main") else {
        return Ok((0.0, 0.0));
    };
    let window = main.window();
    let scale = window.scale_factor()?;
    let bounds = main.bounds()?;
    let position = bounds.position.to_logical::<f64>(scale);
    let size = bounds.size.to_logical::<f64>(scale);
    let inner = window.inner_size()?.to_logical::<f64>(scale);
    let outer = window.outer_size()?.to_logical::<f64>(scale);
    let inset = (size.height - viewport_height).max(0.0);
    tracing::debug!(
        main_x = position.x,
        main_y = position.y,
        main_w = size.width,
        main_h = size.height,
        inner_h = inner.height,
        outer_h = outer.height,
        viewport_height,
        inset,
        "main webview geometry"
    );
    Ok((position.x, position.y + inset))
}

/// Coordinates are logical pixels in the frontend's viewport;
/// `viewport_height` is its `window.innerHeight`.
pub fn show(
    app: &AppHandle,
    url: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    viewport_height: f64,
) -> AppResult<()> {
    let target =
        Url::parse(&url).map_err(|e| AppError::Other(format!("bad PR url {url:?}: {e}")))?;
    let label = label_for(&target);
    let (dx, dy) = main_origin(app, viewport_height)?;
    tracing::debug!(dx, dy, x, y, width, height, "placing PR webview");
    let position = LogicalPosition::new(x + dx, y + dy);
    let size = LogicalSize::new(width, height);

    let mut live = LIVE.lock().unwrap_or_else(|p| p.into_inner());

    for other in live.iter().filter(|l| **l != label) {
        if let Some(webview) = app.get_webview(other) {
            webview.hide()?;
        }
    }

    live.retain(|l| *l != label);
    live.push(label.clone());

    if let Some(webview) = app.get_webview(&label) {
        webview.set_position(position)?;
        webview.set_size(size)?;
        webview.show()?;
        return Ok(());
    }

    // Child webviews hang off the plain `Window` handle, not the `WebviewWindow`.
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

    while live.len() > MAX_LIVE {
        let oldest = live.remove(0);
        if let Some(webview) = app.get_webview(&oldest) {
            webview.close()?;
        }
    }
    Ok(())
}

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

    /// A scheme with no authority is a cannot-be-a-base URL; `url` must still
    /// parse its query.
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
