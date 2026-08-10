//! ANSI terminal color rendering for ffs CLI output.
//!
//! The decision of *whether* to emit color lives in [`super::emit`]: stdout is
//! a tty AND `NO_COLOR` is unset. The render closures in each command always
//! build the colored string; `emit()` strips ANSI when color is off, so piped
//! output and `--format json` stay byte-clean.
//!
//! Color choices follow ripgrep's defaults (`path: magenta`, `line: green`,
//! `match: red + bold`) which read well on both light and dark themes.

use std::io::Write;
use termcolor::{Color, ColorSpec, WriteColor};

/// The default color spec for a matched substring (red + bold).
pub(crate) fn match_spec() -> ColorSpec {
    let mut spec = ColorSpec::new();
    spec.set_fg(Some(Color::Red)).set_bold(true);
    spec
}

/// The default color spec for a file path (magenta; cyan on Windows).
pub(crate) fn path_spec() -> ColorSpec {
    let mut spec = ColorSpec::new();
    spec.set_fg(Some(if cfg!(windows) {
        Color::Cyan
    } else {
        Color::Magenta
    }));
    spec
}

/// The default color spec for a line number (green).
pub(crate) fn line_spec() -> ColorSpec {
    let mut spec = ColorSpec::new();
    spec.set_fg(Some(Color::Green));
    spec
}

/// Return `text` with each `(start, end)` byte range wrapped in `spec` ANSI
/// codes. `ranges` are byte offsets `[start, end)` into `text` (as produced by
/// `GrepMatch.match_byte_offsets`). Ranges are clamped to `text.len()` and
/// overlapping / adjacent ranges are coalesced. The result is plain text when
/// `ranges` is empty or `text` is empty.
///
/// This function never touches the terminal; it only builds the ANSI string.
pub(crate) fn colorize_matches(text: &str, ranges: &[(u32, u32)]) -> String {
    colorize_matches_with(text, ranges, &match_spec())
}

/// Like [`colorize_matches`] but with an explicit color spec.
pub(crate) fn colorize_matches_with(text: &str, ranges: &[(u32, u32)], spec: &ColorSpec) -> String {
    if text.is_empty() || ranges.is_empty() {
        return text.to_string();
    }
    let mut buf = termcolor::Ansi::new(Vec::new());
    // Write colored into a stack buffer, then collect to String.
    write_highlighted(&mut buf, text.as_bytes(), ranges, spec);
    String::from_utf8(buf.into_inner()).unwrap_or_else(|_| text.to_string())
}

/// Write `text` to `buf`, wrapping the byte ranges `ranges` (relative to
/// `text`) in `spec`. Used by renderers that stream through a `WriteColor`.
pub(crate) fn write_highlighted<W: WriteColor>(
    buf: &mut W,
    text: &[u8],
    ranges: &[(u32, u32)],
    spec: &ColorSpec,
) {
    if text.is_empty() || ranges.is_empty() {
        let _ = buf.write_all(text);
        return;
    }

    let merged = normalize_ranges(std::str::from_utf8(text).unwrap_or(""), ranges);
    let mut cursor = 0usize;
    for (s, e) in merged {
        if s > cursor {
            let _ = buf.write_all(&text[cursor..s]);
        }
        if e > s {
            let _ = buf.set_color(spec);
            let _ = buf.write_all(&text[s..e]);
            let _ = buf.reset();
        }
        cursor = cursor.max(e);
    }
    if cursor < text.len() {
        let _ = buf.write_all(&text[cursor..]);
    }
}

/// Clamp, sort and coalesce `ranges` into non-overlapping `[start, end)` byte
/// spans within `[0, len]`. Overlapping / adjacent ranges are merged. Spans are
/// then snapped outward to UTF-8 char boundaries of `text` so slicing never
/// produces invalid UTF-8.
fn normalize_ranges(text: &str, ranges: &[(u32, u32)]) -> Vec<(usize, usize)> {
    let len = text.len();
    let mut spans: Vec<(usize, usize)> = ranges
        .iter()
        .map(|&(s, e)| {
            let s = (s as usize).min(len);
            let e = (e as usize).min(len);
            (s, e.max(s))
        })
        .collect();
    spans.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(spans.len());
    for (s, e) in spans {
        match merged.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    // Snap to char boundaries: shrink the start down, grow the end up.
    let mut snapped: Vec<(usize, usize)> = Vec::with_capacity(merged.len());
    for (s, e) in merged {
        let mut s = s;
        while s > 0 && s < len && !text.is_char_boundary(s) {
            s -= 1;
        }
        let mut e = e;
        while e < len && !text.is_char_boundary(e) {
            e += 1;
        }
        snapped.push((s, e));
    }
    snapped
}

/// Colorize `text` with `base` as the background color, then overlay `ranges`
/// (byte offsets into `text`) in `highlight`. Used to color a whole field (e.g.
/// a path) while drawing attention to the fuzzy-matched substring.
pub(crate) fn colorize_with_base(
    text: &str,
    ranges: &[(u32, u32)],
    base: &ColorSpec,
    highlight: &ColorSpec,
) -> String {
    if text.is_empty() {
        return String::new();
    }
    if ranges.is_empty() {
        return colorize(text, base);
    }
    let merged = normalize_ranges(text, ranges);
    let bytes = text.as_bytes();
    let mut buf = termcolor::Ansi::new(Vec::new());
    let mut cursor = 0usize;
    for (s, e) in merged {
        if s > cursor {
            let _ = buf.set_color(base);
            let _ = buf.write_all(&bytes[cursor..s]);
        }
        if e > s {
            let _ = buf.set_color(highlight);
            let _ = buf.write_all(&bytes[s..e]);
        }
        cursor = cursor.max(e);
    }
    if cursor < text.len() {
        let _ = buf.set_color(base);
        let _ = buf.write_all(&bytes[cursor..]);
    }
    let _ = buf.reset();
    String::from_utf8(buf.into_inner()).unwrap_or_else(|_| text.to_string())
}

/// Colorize the entire `text` with `spec` (no range granularity). Used for
/// whole-field coloring such as the file path and line number.
pub(crate) fn colorize(text: &str, spec: &ColorSpec) -> String {
    if text.is_empty() {
        return text.to_string();
    }
    let mut buf = termcolor::Ansi::new(Vec::new());
    let _ = buf.set_color(spec);
    let _ = buf.write_all(text.as_bytes());
    let _ = buf.reset();
    String::from_utf8(buf.into_inner()).unwrap_or_else(|_| text.to_string())
}

/// Strip ANSI escape sequences (SGR `\x1b[...m` and any CSI/OSC sequence) from
/// `s`. Backstop so piped output is clean even if a render closure leaked
/// color; `emit()` uses this when stdout is not a tty or `NO_COLOR` is set.
pub(crate) fn strip_ansi(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == 0x1b {
            // Skip the ESC and the introducer byte, then the parameter bytes
            // and the final 0x40..=0x7e byte.
            let mut j = i + 1;
            if j < bytes.len() && matches!(bytes[j], b'[' | b']' | b'(' | b')') {
                j += 1;
                while j < bytes.len() && (0x20..=0x3f).contains(&bytes[j]) {
                    j += 1;
                }
                if j < bytes.len() && (0x40..=0x7e).contains(&bytes[j]) {
                    j += 1;
                }
            } else if j < bytes.len() {
                j += 1;
            }
            i = j;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Whether ANSI color should be emitted for the current stdout: it is a tty
/// and `NO_COLOR` is unset. Mirrors the no-color.org convention — any
/// `NO_COLOR` value (including empty) disables color.
pub(crate) fn color_enabled() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_removes_sgr() {
        assert_eq!(strip_ansi("\x1b[31;1mfoo\x1b[0m"), "foo");
        assert_eq!(strip_ansi("plain"), "plain");
        assert_eq!(strip_ansi("\x1b[0m\x1b[1m"), "");
        assert_eq!(strip_ansi("a\x1b[35mb\x1b[0mc"), "abc");
    }

    #[test]
    fn strip_ansi_handles_multi_byte_utf8() {
        assert_eq!(strip_ansi("héllo\x1b[0m"), "héllo");
    }

    #[test]
    fn colorize_empty_input() {
        assert_eq!(colorize_matches("", &[]), "");
        assert_eq!(colorize_matches("abc", &[]), "abc");
    }

    #[test]
    fn colorize_clamps_out_of_bounds() {
        // Offsets beyond text length must not panic; clamped to len.
        let s = colorize_matches("abc", &[(10, 20)]);
        assert_eq!(s, "abc");
    }

    #[test]
    fn colorize_empty_range_noop() {
        // start == end must not panic and must not emit color.
        let s = colorize_matches("abc", &[(1, 1)]);
        assert_eq!(s, "abc");
    }

    #[test]
    fn colorize_merges_overlapping() {
        // (1,3) and (2,4) overlap → single colored run, round-trips clean.
        let s = colorize_matches("abcdef", &[(1, 3), (2, 4)]);
        assert!(s.contains("\x1b["));
        assert_eq!(strip_ansi(&s), "abcdef");
    }

    #[test]
    fn colorize_utf8_byte_ranges() {
        // "héllo" — 'é' is 2 bytes. Highlighting bytes that split the 'é'
        // must not panic or drop color; snap to char boundaries.
        let s = colorize_matches("héllo", &[(2, 5)]);
        assert!(s.contains("\x1b["));
        assert_eq!(strip_ansi(&s), "héllo");
    }

    #[test]
    fn colorize_splits_multibyte_char_gracefully() {
        // Range (1,3) splits the 2-byte 'é' at index 1..3. Must not panic and
        // round-trips to the full string.
        let s = colorize_matches("héllo", &[(1, 3)]);
        assert_eq!(strip_ansi(&s), "héllo");
        // The whole 'é' ends up inside a single colored span.
        assert!(s.contains("\x1b["));
    }

    #[test]
    fn colorize_with_base_overlays() {
        // Base path magenta, fuzzy match red overlay. Path "/src/foo.rs",
        // highlight bytes [5,8) = "foo".
        let s = colorize_with_base("/src/foo.rs", &[(5, 8)], &path_spec(), &match_spec());
        assert_eq!(strip_ansi(&s), "/src/foo.rs");
        assert!(s.contains("\x1b["));
    }

    #[test]
    fn colorize_with_base_empty_ranges_uses_base() {
        let s = colorize_with_base("/src/foo.rs", &[], &path_spec(), &match_spec());
        assert_eq!(strip_ansi(&s), "/src/foo.rs");
        assert!(s.contains("\x1b["));
    }
}
