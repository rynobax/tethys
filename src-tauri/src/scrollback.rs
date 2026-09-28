//! Recorded output contains the queries a program asked the terminal at
//! startup. Replayed into a fresh xterm.js, each gets answered again, and the
//! stale replies land in the program's input. Only replayed bytes are
//! stripped: a live query is one something is waiting on.

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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn nothing_in_nothing_out() {
        assert_eq!(strip(""), "");
    }
}
