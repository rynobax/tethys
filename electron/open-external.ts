import { shell } from "electron";

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
