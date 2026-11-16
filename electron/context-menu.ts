import {
  clipboard,
  Menu,
  type BrowserWindow,
  type ContextMenuParams,
  type MenuItemConstructorOptions,
  type WebContents,
} from "electron";

import { openExternal } from "./open-external";

/**
 * The right-click menu is Chrome the browser's, not the engine's, so Electron
 * shows none unless the app builds one. `contents` is whatever was clicked in:
 * a PR's guest, or the window's own page and the artifact frames inside it.
 */
export function popupContextMenu(
  contents: WebContents,
  params: ContextMenuParams,
  opts: { window: BrowserWindow | null; navigation: boolean },
) {
  Menu.buildFromTemplate(template(contents, params, opts.navigation)).popup({
    window: opts.window ?? undefined,
  });
}

/** Chrome's sections, in Chrome's order; back, forward and reload only where
 *  asked for and when nothing more specific was clicked. */
function template(
  guest: WebContents,
  params: ContextMenuParams,
  navigation: boolean,
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
  if (navigation && sections.length === 0) {
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
  // An artifact's frame inside the window's page: the page itself reloads
  // from the View menu.
  const frame = params.frame;
  if (!navigation && frame && frame !== guest.mainFrame) {
    sections.push([{ label: "Reload Frame", click: () => frame.reload() }]);
  }
  sections.push([
    { label: "Inspect Element", click: () => guest.inspectElement(x, y) },
  ]);

  return sections.flatMap((section, i) =>
    i === 0 ? section : [{ type: "separator" as const }, ...section],
  );
}
