import {
  BrowserWindow,
  clipboard,
  Menu,
  shell,
  type ContextMenuParams,
  type MenuItemConstructorOptions,
  type WebContents,
} from "electron";

/**
 * The PR tab's `<webview>`s (`src/PrViews.tsx`). They only ever load GitHub,
 * with no preload, so a page has no path to the app's bridge or to Node.
 */

const GITHUB = "github.com";
const PARTITION = "persist:github";

/** GitHub's sign-in paths, kept in the view along with the PR itself. */
const SIGN_IN = [
  "/login",
  "/session",
  "/sessions",
  "/sso",
  "/saml",
  "/oauth",
  "/password_reset",
  "/settings/two_factor",
];

/** The one way out to the default browser, for http(s) only. A test run
 *  (`TETHYS_TEST`) logs instead, so it never takes the user's focus. */
export function openExternal(url: string) {
  let protocol: string;
  try {
    protocol = new URL(url).protocol;
  } catch {
    return;
  }
  if (protocol !== "http:" && protocol !== "https:") return;
  if (process.env.TETHYS_TEST) {
    console.log(`openExternal: ${url}`);
    return;
  }
  shell.openExternal(url).catch((e) => console.error("openExternal:", e));
}

/**
 * Keeps links within the PR and GitHub's sign-in paths in the view and sends
 * the rest to the browser. A click handler because GitHub's Turbo links use
 * `pushState`, which no navigation event sees; it ignores other hosts so an
 * SSO provider's pages are left alone mid-login. Leaving goes through
 * `window.open`, which the guest's window-open handler sends to the browser.
 */
function linkPolicyScript(pr: URL): string {
  const host = JSON.stringify(pr.host);
  const stay = JSON.stringify([pr.pathname, ...SIGN_IN]);
  return `(() => {
  const PR_HOST = ${host};
  const STAY = ${stay};
  const under = (p, base) => p === base || p.startsWith(base + "/");
  const stays = (u) =>
    u.host === PR_HOST &&
    (STAY.some((base) => under(u.pathname, base)) ||
      (u.pathname.startsWith("/orgs/") && u.pathname.includes("/sso")));
  document.addEventListener("click", (e) => {
    if (e.defaultPrevented || e.button !== 0) return;
    if (location.host !== PR_HOST) return;
    const a = e.target instanceof Element ? e.target.closest("a[href]") : null;
    if (!a) return;
    let u;
    try { u = new URL(a.href, location.href); } catch { return; }
    if (u.protocol !== "http:" && u.protocol !== "https:") return;
    if (stays(u)) return;
    e.preventDefault();
    e.stopImmediatePropagation();
    window.open(u.href, "_blank", "noopener");
  }, true);
})();`;
}

/** Where an SSO flow may hand the page to another host. */
function signingIn(u: URL): boolean {
  return (
    u.host === GITHUB &&
    (SIGN_IN.some((b) => u.pathname === b || u.pathname.startsWith(b + "/")) ||
      (u.pathname.startsWith("/orgs/") &&
        (u.pathname.includes("/sso") || u.pathname.includes("/saml"))))
  );
}

/** Applied to the main window's contents, the only embedder. */
export function policePrViews(embedder: WebContents) {
  // `did-attach-webview` doesn't carry the `src`; it follows its
  // `will-attach-webview` synchronously.
  let attaching: URL | null = null;

  embedder.on("will-attach-webview", (event, prefs, params) => {
    let src: URL;
    try {
      src = new URL(params.src);
    } catch {
      event.preventDefault();
      return;
    }
    if (src.protocol !== "https:" || src.host !== GITHUB) {
      console.error("refused a webview for", params.src);
      event.preventDefault();
      return;
    }
    delete prefs.preload;
    prefs.nodeIntegration = false;
    prefs.nodeIntegrationInSubFrames = false;
    prefs.contextIsolation = true;
    prefs.sandbox = true;
    prefs.webSecurity = true;
    params.partition = PARTITION;
    attaching = src;
  });

  embedder.on("did-attach-webview", (_event, guest) => {
    const pr = attaching;
    attaching = null;
    if (!pr) {
      guest.close();
      return;
    }
    guest.setWindowOpenHandler(({ url }) => {
      openExternal(url);
      return { action: "deny" };
    });
    // The click handler covers links; this catches script navigations off
    // GitHub, except mid sign-in, where an SSO provider may take over.
    guest.on("will-navigate", (event) => {
      const to = new URL(event.url);
      if (to.protocol !== "http:" && to.protocol !== "https:") {
        event.preventDefault();
        return;
      }
      let from: URL | null = null;
      try {
        from = new URL(guest.getURL());
      } catch {}
      if (from?.host === GITHUB && to.host !== GITHUB && !signingIn(from)) {
        event.preventDefault();
        openExternal(to.href);
      }
    });
    // Electron shows none unless the app builds one.
    guest.on("context-menu", (_event, params) => {
      Menu.buildFromTemplate(contextMenu(guest, params)).popup({
        window: BrowserWindow.fromWebContents(embedder) ?? undefined,
      });
    });
    const script = linkPolicyScript(pr);
    guest.on("dom-ready", () => {
      guest.executeJavaScript(script).catch((e) => {
        console.error("PR link policy failed to install:", e);
      });
    });
  });
}

/** Chrome's sections, in Chrome's order; the page's own only when nothing
 *  more specific was clicked. */
function contextMenu(
  guest: WebContents,
  params: ContextMenuParams,
): MenuItemConstructorOptions[] {
  const { editFlags: can, x, y } = params;
  const sections: MenuItemConstructorOptions[][] = [];

  if (params.linkURL) {
    sections.push([
      {
        label: "Open Link in Browser",
        click: () => openExternal(params.linkURL),
      },
      {
        label: "Copy Link Address",
        click: () => clipboard.writeText(params.linkURL),
      },
    ]);
  }
  if (params.mediaType === "image" && params.srcURL) {
    sections.push([
      { label: "Copy Image", click: () => guest.copyImageAt(x, y) },
      {
        label: "Copy Image Address",
        click: () => clipboard.writeText(params.srcURL),
      },
    ]);
  }
  if (params.isEditable) {
    if (params.misspelledWord) {
      const word = params.misspelledWord;
      sections.push([
        ...(params.dictionarySuggestions.length > 0
          ? params.dictionarySuggestions.map((s) => ({
              label: s,
              click: () => guest.replaceMisspelling(s),
            }))
          : [{ label: "No Guesses Found", enabled: false }]),
        {
          label: "Add to Dictionary",
          click: () => guest.session.addWordToSpellCheckerDictionary(word),
        },
      ]);
    }
    sections.push([
      { label: "Cut", enabled: can.canCut, click: () => guest.cut() },
      { label: "Copy", enabled: can.canCopy, click: () => guest.copy() },
      { label: "Paste", enabled: can.canPaste, click: () => guest.paste() },
      {
        label: "Select All",
        enabled: can.canSelectAll,
        click: () => guest.selectAll(),
      },
    ]);
  } else if (params.selectionText.trim()) {
    sections.push([{ label: "Copy", click: () => guest.copy() }]);
  }
  if (sections.length === 0) {
    const history = guest.navigationHistory;
    sections.push([
      {
        label: "Back",
        enabled: history.canGoBack(),
        click: () => history.goBack(),
      },
      {
        label: "Forward",
        enabled: history.canGoForward(),
        click: () => history.goForward(),
      },
      { label: "Reload", click: () => guest.reload() },
    ]);
  }
  sections.push([
    { label: "Inspect Element", click: () => guest.inspectElement(x, y) },
  ]);

  return sections.flatMap((section, i) =>
    i === 0 ? section : [{ type: "separator" as const }, ...section],
  );
}
