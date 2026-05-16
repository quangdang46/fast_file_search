//! Comment / whitespace filtering at three intensity levels.
//!
//! * [`NoFilter`] — passthrough.
//! * [`MinimalFilter`] — strip line comments and trailing whitespace; preserves
//!   doc comments.
//! * [`AggressiveFilter`] — strip line, block and doc comments and collapse blank lines.

use std::path::Path;

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

/// Selected filter intensity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FilterLevel {
    /// No transformation.
    None,
    /// Strip line comments and trailing whitespace; keep doc comments and blank lines.
    #[default]
    Minimal,
    /// Strip line, block and doc comments; collapse runs of blank lines to one.
    Aggressive,
}

/// Auto-pick a filter based on file extension. Test files default to `None`
/// because deleting comments often changes test intent (snapshots, descriptions).
#[must_use]
pub fn detect_filter_level(path: &Path, level: FilterLevel) -> FilterLevel {
    if level == FilterLevel::None {
        return FilterLevel::None;
    }
    let path_str = path.to_string_lossy();
    if path_str.contains(".test.") || path_str.contains(".spec.") || path_str.contains("__tests__")
    {
        return FilterLevel::None;
    }
    level
}

/// Trait every concrete filter implements. `apply` consumes a slice and returns
/// the filtered string; concrete impls are stateless.
pub trait FilterStrategy: Send + Sync {
    fn name(&self) -> &'static str;
    fn apply(&self, input: &str) -> String;
}

/// Identity passthrough.
pub struct NoFilter;

impl FilterStrategy for NoFilter {
    fn name(&self) -> &'static str {
        "none"
    }
    fn apply(&self, input: &str) -> String {
        input.to_string()
    }
}

/// Strip line comments (`//`, `#`) and trailing whitespace; preserve doc comments
/// (`///`, `//!`, `#:`) and blank lines.
pub struct MinimalFilter;

impl FilterStrategy for MinimalFilter {
    fn name(&self) -> &'static str {
        "minimal"
    }
    fn apply(&self, input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        for line in input.split_inclusive('\n') {
            let body = strip_line_comment_keep_doc(line);
            let trimmed = trim_trailing_whitespace_keep_newline(&body);
            out.push_str(&trimmed);
        }
        out
    }
}

/// Strip line comments, block comments, and doc comments; collapse multiple
/// blank lines to a single one.
pub struct AggressiveFilter;

impl FilterStrategy for AggressiveFilter {
    fn name(&self) -> &'static str {
        "aggressive"
    }
    fn apply(&self, input: &str) -> String {
        let no_block = strip_block_comments(input);
        let no_line = strip_all_line_comments(&no_block);
        collapse_blank_lines(&no_line)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn strip_line_comment_keep_doc(line: &str) -> String {
    let stripped_newline_len = if line.ends_with("\r\n") {
        2
    } else if line.ends_with('\n') {
        1
    } else {
        0
    };
    let body = &line[..line.len() - stripped_newline_len];
    let newline = &line[line.len() - stripped_newline_len..];

    if let Some(idx) = find_line_comment_start(body) {
        let prefix = &body[..idx];
        // Defensive: idx + 2 may not land on a UTF-8 char boundary if `body`
        // came from `from_utf8_lossy` and the marker byte (e.g. `#`) sits
        // immediately before a multi-byte replacement char. Use raw bytes.
        let marker_bytes = &body.as_bytes()[idx..(idx + 2).min(body.len())];
        let comment_marker = std::str::from_utf8(marker_bytes).unwrap_or("");
        if is_doc_comment_marker(body, idx, comment_marker) {
            return line.to_string();
        }
        return format!("{}{}", prefix.trim_end(), newline);
    }
    line.to_string()
}

fn find_line_comment_start(body: &str) -> Option<usize> {
    let bytes = body.as_bytes();
    let mut in_string: Option<u8> = None;
    let mut i = 0;
    while i + 1 < bytes.len() {
        let b = bytes[i];
        if let Some(q) = in_string {
            if b == b'\\' {
                i += 2;
                continue;
            }
            if b == q {
                in_string = None;
            }
            i += 1;
            continue;
        }
        if matches!(b, b'"' | b'\'' | b'`') {
            in_string = Some(b);
            i += 1;
            continue;
        }
        if b == b'/' && bytes[i + 1] == b'/' {
            return Some(i);
        }
        if b == b'#' && i + 1 < bytes.len() {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn is_doc_comment_marker(body: &str, idx: usize, marker: &str) -> bool {
    if marker == "//" {
        let next = body.as_bytes().get(idx + 2);
        return matches!(next, Some(b'/') | Some(b'!'));
    }
    if marker.starts_with('#') {
        let next = body.as_bytes().get(idx + 1);
        return matches!(next, Some(b':') | Some(b'!'));
    }
    false
}

fn trim_trailing_whitespace_keep_newline(line: &str) -> String {
    let stripped_newline_len = if line.ends_with("\r\n") {
        2
    } else if line.ends_with('\n') {
        1
    } else {
        0
    };
    let body = &line[..line.len() - stripped_newline_len];
    let newline = &line[line.len() - stripped_newline_len..];
    let mut s = body.trim_end().to_string();
    s.push_str(newline);
    s
}

static BLOCK_COMMENT_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?s)/\*.*?\*/").expect("block comment regex compiles"));

fn strip_block_comments(s: &str) -> String {
    BLOCK_COMMENT_RE.replace_all(s, "").into_owned()
}

fn strip_all_line_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for line in input.split_inclusive('\n') {
        let stripped_newline_len = if line.ends_with("\r\n") {
            2
        } else if line.ends_with('\n') {
            1
        } else {
            0
        };
        let body = &line[..line.len() - stripped_newline_len];
        let newline = &line[line.len() - stripped_newline_len..];
        if let Some(idx) = find_line_comment_start(body) {
            out.push_str(body[..idx].trim_end());
            out.push_str(newline);
        } else {
            out.push_str(line);
        }
    }
    out
}

fn collapse_blank_lines(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_blank = false;
    for line in input.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        if body.trim().is_empty() {
            if last_blank {
                continue;
            }
            last_blank = true;
        } else {
            last_blank = false;
        }
        out.push_str(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_skips_test_files() {
        assert_eq!(
            detect_filter_level(Path::new("foo.test.ts"), FilterLevel::Minimal),
            FilterLevel::None
        );
        assert_eq!(
            detect_filter_level(Path::new("__tests__/x.ts"), FilterLevel::Aggressive),
            FilterLevel::None
        );
        assert_eq!(
            detect_filter_level(Path::new("src/lib.rs"), FilterLevel::Minimal),
            FilterLevel::Minimal
        );
    }

    #[test]
    fn no_filter_preserves_everything() {
        let f = NoFilter;
        let input = "// hi\n  fn foo() {} // bar\n";
        assert_eq!(f.apply(input), input);
    }

    #[test]
    fn minimal_strips_line_comments_keeps_doc() {
        let f = MinimalFilter;
        let input = "/// doc\nfn foo() {} // strip me\n";
        let out = f.apply(input);
        assert!(out.contains("/// doc"));
        assert!(!out.contains("strip me"));
    }

    #[test]
    fn minimal_keeps_python_pragma() {
        let f = MinimalFilter;
        let input = "#:noqa pragma\nx = 1\n";
        let out = f.apply(input);
        assert!(out.contains("#:noqa pragma"));
    }

    #[test]
    fn aggressive_strips_block_and_collapses_blanks() {
        let f = AggressiveFilter;
        let input = "/* block */\n\n\nfn foo() {}\n\n\n\nfn bar() {}\n";
        let out = f.apply(input);
        assert!(!out.contains("block"));
        // No more than 1 blank line in a row.
        assert!(!out.contains("\n\n\n"));
    }

    #[test]
    fn line_comment_inside_string_is_kept() {
        let f = MinimalFilter;
        let input = "let url = \"https://example.com\";\n";
        let out = f.apply(input);
        assert!(out.contains("https://example.com"));
    }
}
