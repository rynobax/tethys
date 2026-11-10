//! Replaying recorded output into a fresh xterm.js has two hazards.
//!
//! It contains the queries a program asked the terminal at startup. Replayed,
//! each gets answered again, and the stale replies land in the program's
//! input. Only replayed bytes are stripped: a live query is one something is
//! waiting on.
//!
//! And it may no longer contain the mode switches. tmux turns mouse reporting
//! on once, when its client attaches (and again on a resize), never while
//! output streams; once the ring has rolled past that, a replay leaves xterm
//! believing the mouse is its own, so the wheel scrolls xterm's buffer instead
//! of reaching tmux. `TermModes` remembers the last state of each mode that
//! matters, to be re-asserted after the replay.

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
                // Truncated at the ring's edge.
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

/// Index of a CSI's final byte, starting just past `ESC [`.
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

/// `seq` is the CSI after `ESC [`, final byte included.
fn is_csi_query(seq: &[u8]) -> bool {
    let (&final_byte, params) = match seq.split_last() {
        Some(parts) => parts,
        None => return false,
    };
    match final_byte {
        // Device Attributes and Device Status Report.
        b'c' | b'n' => true,
        // Kitty keyboard: `ESC[>1u` push and `ESC[<u` pop are state to keep.
        b'u' => params == b"?",
        // XTVERSION; `ESC[ q` is DECSCUSR (cursor shape) and must survive.
        b'q' => params.first() == Some(&b'>'),
        _ => false,
    }
}

/// `(body_end, seq_end)` of an OSC starting just past `ESC ]`, terminated by
/// BEL or `ESC \`.
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

/// Colour queries (`10;?`, `11;?`, `4;<n>;?`) put `?` where a set would put
/// the value.
fn is_osc_query(body: &[u8]) -> bool {
    body.rsplit(|&b| b == b';').next() == Some(b"?")
}

/// DEC private modes that change how the terminal talks back: cursor keys,
/// cursor visibility, every mouse protocol, focus events, bracketed paste.
/// Not the alternate screen, whose switch would clear what was just replayed.
const TRACKED_MODES: &[u16] = &[1, 25, 1000, 1002, 1003, 1004, 1005, 1006, 1015, 2004];

/// Longest `ESC [ ? … h` worth carrying across a read boundary.
const MAX_CARRY: usize = 64;

#[derive(Debug, Default)]
pub struct TermModes {
    /// In the order first seen, so a restore replays them in a stable order.
    modes: Vec<(u16, bool)>,
    carry: Vec<u8>,
}

impl TermModes {
    pub fn observe(&mut self, chunk: &[u8]) {
        let mut window = std::mem::take(&mut self.carry);
        window.extend_from_slice(chunk);
        let mut i = 0;
        while let Some(at) = find(&window[i..], b"\x1b[?").map(|p| p + i) {
            let params_start = at + 3;
            let mut j = params_start;
            while j < window.len() && (window[j].is_ascii_digit() || window[j] == b';') {
                j += 1;
            }
            let Some(&final_byte) = window.get(j) else {
                if window.len() - at <= MAX_CARRY {
                    self.carry = window[at..].to_vec();
                }
                return;
            };
            if final_byte == b'h' || final_byte == b'l' {
                for param in window[params_start..j].split(|&b| b == b';') {
                    let Some(mode) = std::str::from_utf8(param)
                        .ok()
                        .and_then(|p| p.parse::<u16>().ok())
                    else {
                        continue;
                    };
                    self.set(mode, final_byte == b'h');
                }
            }
            i = j;
        }
        // A lone trailing ESC, or `ESC [`, may begin the next one.
        let tail = window.len().saturating_sub(2);
        if let Some(p) = window[tail..].iter().position(|&b| b == ESC) {
            self.carry = window[tail + p..].to_vec();
        }
    }

    fn set(&mut self, mode: u16, on: bool) {
        if !TRACKED_MODES.contains(&mode) {
            return;
        }
        match self.modes.iter_mut().find(|(m, _)| *m == mode) {
            Some(entry) => entry.1 = on,
            None => self.modes.push((mode, on)),
        }
    }

    /// Sequences that put a terminal in the last state seen.
    pub fn restore(&self) -> Vec<u8> {
        self.modes
            .iter()
            .flat_map(|(mode, on)| format!("\x1b[?{mode}{}", if *on { 'h' } else { 'l' }).into_bytes())
            .collect()
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modes_after(chunks: &[&str]) -> String {
        let mut modes = TermModes::default();
        for chunk in chunks {
            modes.observe(chunk.as_bytes());
        }
        String::from_utf8(modes.restore()).unwrap()
    }

    #[test]
    fn the_last_state_of_each_mode_is_restored() {
        assert_eq!(
            modes_after(&["\x1b[?1000h\x1b[?1006hhello\x1b[?2004h", "\x1b[?2004l"]),
            "\x1b[?1000h\x1b[?1006h\x1b[?2004l"
        );
    }

    #[test]
    fn combined_params_each_count() {
        assert_eq!(modes_after(&["\x1b[?1000;1006h"]), "\x1b[?1000h\x1b[?1006h");
    }

    #[test]
    fn a_sequence_split_across_reads_is_seen() {
        assert_eq!(modes_after(&["out\x1b", "[?10", "06h"]), "\x1b[?1006h");
        assert_eq!(modes_after(&["out\x1b[", "?1000h"]), "\x1b[?1000h");
    }

    #[test]
    fn the_alternate_screen_and_other_csis_are_ignored() {
        assert_eq!(modes_after(&["\x1b[?1049h\x1b[2J\x1b[?7h\x1b[1;32m"]), "");
    }

    fn strip(s: &str) -> String {
        String::from_utf8(strip_queries(s.as_bytes())).unwrap()
    }

    #[test]
    fn codex_startup_queries_are_removed() {
        let startup = "\x1b[?2004h\x1b[>4;0m\x1b[>5u\x1b[?1004h\x1b[6n\x1b]10;?\x1b\\\x1b]11;?\x1b\\\x1b[?u\x1b[c";
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

    #[test]
    fn ordinary_output_is_untouched() {
        for seq in [
            "plain text\r\n",
            "\x1b[1;32mgreen\x1b[0m",
            "\x1b[2J\x1b[H",
            "\x1b[10C\x1b[5A",
            "\x1b[?2004h",
            "\x1b[?1004h",
            "\x1b]0;a window title\x07",
            "\x1b]8;;https://example.com\x07link\x1b]8;;\x07",
        ] {
            assert_eq!(strip(seq), seq, "mangled {seq:?}");
        }
    }

    #[test]
    fn set_commands_that_look_like_queries_survive() {
        assert_eq!(strip("\x1b[0 q"), "\x1b[0 q");
        assert_eq!(strip("\x1b[2 q"), "\x1b[2 q");
        assert_eq!(strip("\x1b[>1u"), "\x1b[>1u");
        assert_eq!(strip("\x1b[<u"), "\x1b[<u");
        assert_eq!(strip("\x1b[>0q"), "");
    }

    #[test]
    fn a_sequence_cut_off_at_the_ring_edge_is_kept() {
        assert_eq!(strip("output\x1b[38;5;"), "output\x1b[38;5;");
        assert_eq!(strip("output\x1b]11;"), "output\x1b]11;");
        assert_eq!(strip("output\x1b"), "output\x1b");
    }

}
