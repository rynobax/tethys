//! Making recorded PTY output safe to replay into a fresh terminal.
//!
//! The scrollback ring holds raw bytes, which is what makes a reattached pane
//! look exactly as it did. But raw output contains the *questions* the program
//! asked the terminal at startup — "where is the cursor", "what are you",
//! "what colour is your background" — and a terminal emulator answers a
//! question whenever it reads one. Replay the ring into a newly-mounted
//! xterm.js and it answers all of them again, weeks late, up the PTY and into
//! whatever the program is now showing. In codex that surfaced as `0;276;0c`
//! typed into the composer every time you switched workspaces: the tail of
//! xterm.js's secondary-device-attributes reply.
//!
//! So the replayed copy has the queries taken out. Only the replayed copy —
//! live bytes stream through untouched, because a query the program asks *now*
//! is one it is waiting for an answer to.
//!
//! Dropping them costs nothing on screen: a query renders nothing. It is the
//! reply that was never wanted.

/// Remove terminal queries from recorded output, leaving everything else byte
/// for byte.
pub fn strip_queries(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] != ESC || i + 1 >= input.len() {
            out.push(input[i]);
            i += 1;
            continue;
        }
        match input[i + 1] {
            b'[' => match csi_end(input, i + 2) {
                Some(end) => {
                    if !is_csi_query(&input[i + 2..=end]) {
                        out.extend_from_slice(&input[i..=end]);
                    }
                    i = end + 1;
                }
                // Truncated at the ring's edge: keep it and stop guessing.
                None => {
                    out.extend_from_slice(&input[i..]);
                    break;
                }
            },
            b']' => match osc_end(input, i + 2) {
                Some((body_end, seq_end)) => {
                    if !is_osc_query(&input[i + 2..body_end]) {
                        out.extend_from_slice(&input[i..=seq_end]);
                    }
                    i = seq_end + 1;
                }
                None => {
                    out.extend_from_slice(&input[i..]);
                    break;
                }
            },
            _ => {
                out.push(input[i]);
                i += 1;
            }
        }
    }
    out
}

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;

/// Index of a CSI sequence's final byte, starting from just past `ESC [`.
///
/// A CSI is parameter bytes (`0x30..=0x3f`), then intermediates
/// (`0x20..=0x2f`), then one final byte (`0x40..=0x7e`).
fn csi_end(input: &[u8], start: usize) -> Option<usize> {
    let mut i = start;
    while i < input.len() && (0x30..=0x3f).contains(&input[i]) {
        i += 1;
    }
    while i < input.len() && (0x20..=0x2f).contains(&input[i]) {
        i += 1;
    }
    match input.get(i) {
        Some(&b) if (0x40..=0x7e).contains(&b) => Some(i),
        _ => None,
    }
}

/// Whether a CSI — given its bytes after `ESC [`, final byte included — asks
/// the terminal something.
fn is_csi_query(seq: &[u8]) -> bool {
    let (&final_byte, params) = match seq.split_last() {
        Some(parts) => parts,
        None => return false,
    };
    match final_byte {
        // Device Attributes: `ESC[c`, `ESC[>c`, `ESC[=c`. The reply is what
        // arrived in the composer as `0;276;0c`.
        b'c' => true,
        // Device Status Report, including `ESC[6n` for the cursor position.
        b'n' => true,
        // Kitty keyboard: `ESC[?u` asks for the current flags. The other `u`
        // forms — `ESC[>1u` to push, `ESC[<u` to pop — are commands, and a
        // pushed flag stack is part of the state a replay should restore.
        b'u' => params == b"?",
        // XTVERSION (`ESC[>q`) asks the terminal to name itself. Bare
        // `ESC[ q` with a space intermediate is DECSCUSR, which sets the
        // cursor shape and must survive.
        b'q' => params.first() == Some(&b'>'),
        _ => false,
    }
}

/// The end of an OSC starting just past `ESC ]`, as `(body_end, seq_end)` —
/// the body excludes the terminator, which is either BEL or `ESC \`.
fn osc_end(input: &[u8], start: usize) -> Option<(usize, usize)> {
    let mut i = start;
    while i < input.len() {
        match input[i] {
            BEL => return Some((i, i)),
            ESC if input.get(i + 1) == Some(&b'\\') => return Some((i, i + 1)),
            _ => i += 1,
        }
    }
    None
}

/// Whether an OSC body asks the terminal something. The colour queries
/// (`10;?` foreground, `11;?` background, `4;<n>;?` palette) all spell the
/// question as a final `?` field, where setting one would carry a value.
fn is_osc_query(body: &[u8]) -> bool {
    body.rsplit(|&b| b == b';').next() == Some(b"?")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip(s: &str) -> String {
        String::from_utf8(strip_queries(s.as_bytes())).unwrap()
    }

    /// The exact burst codex emits at startup. Every one of these got answered
    /// again on each workspace switch.
    #[test]
    fn codex_startup_queries_are_removed() {
        let startup = "\x1b[?2004h\x1b[>4;0m\x1b[>5u\x1b[?1004h\x1b[6n\x1b]10;?\x1b\\\x1b]11;?\x1b\\\x1b[?u\x1b[c";
        // What's left is the state-setting half: bracketed paste, the kitty
        // flags codex pushed, focus reporting.
        assert_eq!(strip(startup), "\x1b[?2004h\x1b[>4;0m\x1b[>5u\x1b[?1004h");
    }

    #[test]
    fn device_attribute_queries_go() {
        assert_eq!(strip("a\x1b[cb"), "ab");
        assert_eq!(strip("a\x1b[>cb"), "ab");
        assert_eq!(strip("a\x1b[=cb"), "ab");
        assert_eq!(strip("a\x1b[0cb"), "ab");
    }

    #[test]
    fn cursor_position_and_status_requests_go() {
        assert_eq!(strip("a\x1b[6nb"), "ab");
        assert_eq!(strip("a\x1b[5nb"), "ab");
        assert_eq!(strip("a\x1b[?6nb"), "ab");
    }

    #[test]
    fn colour_queries_go_with_either_terminator() {
        assert_eq!(strip("a\x1b]11;?\x1b\\b"), "ab");
        assert_eq!(strip("a\x1b]10;?\x07b"), "ab");
        assert_eq!(strip("a\x1b]4;1;?\x07b"), "ab");
    }

    /// The whole point is that only the questions go. Anything that paints,
    /// positions, colours or sets a mode is what makes a replayed pane look
    /// like the one you left.
    #[test]
    fn ordinary_output_is_untouched() {
        for seq in [
            "plain text\r\n",
            "\x1b[1;32mgreen\x1b[0m",
            "\x1b[2J\x1b[H",
            "\x1b[10C\x1b[5A",      // cursor forward / up — capital C, not DA
            "\x1b[?2004h",          // bracketed paste on
            "\x1b[?1004h",          // focus reporting on
            "\x1b]0;a window title\x07",
            "\x1b]8;;https://example.com\x07link\x1b]8;;\x07",
        ] {
            assert_eq!(strip(seq), seq, "mangled {seq:?}");
        }
    }

    /// `ESC[ q` sets the cursor shape and `ESC[>1u` pushes kitty flags — both
    /// are state a replay has to restore. Only the `>`-prefixed `q` and the
    /// bare `?u` are questions.
    #[test]
    fn set_commands_that_look_like_queries_survive() {
        assert_eq!(strip("\x1b[0 q"), "\x1b[0 q");
        assert_eq!(strip("\x1b[2 q"), "\x1b[2 q");
        assert_eq!(strip("\x1b[>1u"), "\x1b[>1u");
        assert_eq!(strip("\x1b[<u"), "\x1b[<u");
        assert_eq!(strip("\x1b[>0q"), "");
    }

    /// The ring is a fixed-size window, so its first bytes are wherever the
    /// buffer happened to wrap — mid-sequence as often as not. A truncated
    /// tail is passed through rather than guessed at: a stray fragment renders
    /// as nothing much, where dropping real output would lose the screen.
    #[test]
    fn a_sequence_cut_off_at_the_ring_edge_is_kept() {
        assert_eq!(strip("output\x1b[38;5;"), "output\x1b[38;5;");
        assert_eq!(strip("output\x1b]11;"), "output\x1b]11;");
        assert_eq!(strip("output\x1b"), "output\x1b");
    }

    #[test]
    fn nothing_in_nothing_out() {
        assert_eq!(strip(""), "");
    }
}
