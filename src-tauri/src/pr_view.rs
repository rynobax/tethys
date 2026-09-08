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
//! Trust boundary: `default.json` scopes its permissions to the `"main"`
//! webview, so these `"pr-view-*"` webviews are denied every *plugin* command
//! (`core:*`, `dialog`, `opener`). Application commands, though, aren't part
//! of Tauri's ACL and stay reachable from any webview with an IPC bridge, so
//! the real safeguard is that these webviews are only ever pointed at GitHub —
//! a trusted origin — and navigation isn't locked down further only so that
//! org SSO login redirects still work.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager, Url, WebviewUrl};

use crate::error::{AppError, AppResult};

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
    window.add_child(
        tauri::webview::WebviewBuilder::new(&label, WebviewUrl::External(target)),
        position,
        size,
    )?;

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
