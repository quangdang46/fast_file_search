//! Shared pure helpers for the grep engine.

use ffs_query_parser::{Constraint, FfsQuery, GrepConfig, QueryParser};

/// Determine whether `text` contains any regex metacharacters.
/// Uses `regex::escape` from the regex crate as the source of truth — if the
/// escaped form differs from the original, the text contains characters that
/// would be interpreted as regex syntax. This is deterministic and always in
/// sync with the regex engine (no hand-rolled heuristic to maintain).
///
/// Callers can use this to choose between `GrepMode::Regex` and
/// `GrepMode::PlainText`. When `Regex` mode is chosen and the pattern turns
/// out to be invalid, `grep_search` already falls back to plain-text matching
/// and populates `regex_fallback_error`.
pub fn has_regex_metacharacters(text: &str) -> bool {
    regex::escape(text) != text
}

/// Check if `text` contains `\n` that is NOT preceded by another `\`.
///
/// `\n` → true (user wants multiline search)
/// `\\n` → false (escaped backslash followed by literal `n`, e.g. `\\nvim-data`)
#[inline]
pub fn has_unescaped_newline_escape(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len().saturating_sub(1) {
        if bytes[i] == b'\\' {
            if bytes[i + 1] == b'n' {
                // Count consecutive backslashes ending at position i
                let mut backslash_count = 1;
                while backslash_count <= i && bytes[i - backslash_count] == b'\\' {
                    backslash_count += 1;
                }
                // Odd number of backslashes before 'n' → real \n escape
                if backslash_count % 2 == 1 {
                    return true;
                }
            }
            // Skip past the escaped character
            i += 2;
        } else {
            i += 1;
        }
    }
    false
}

/// Replace only unescaped `\n` sequences with real newlines.
///
/// `\n` → newline character
/// `\\n` → preserved as-is (literal backslash + `n`)
pub fn replace_unescaped_newline_escapes(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut result = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            if bytes[i + 1] == b'n' {
                let mut backslash_count = 1;
                while backslash_count <= i && bytes[i - backslash_count] == b'\\' {
                    backslash_count += 1;
                }
                if backslash_count % 2 == 1 {
                    result.push(b'\n');
                    i += 2;
                    continue;
                }
            }
            result.push(bytes[i]);
            i += 1;
        } else {
            result.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(result).unwrap_or_else(|_| text.to_string())
}

/// Heuristic: can this regex pattern match a `\n` byte? Used to decide
/// `multi_line` for the searcher — patterns that can span lines via `(?s)`,
/// `\s`, or a negated class `[^…]` count as multiline even without a
/// literal `\n` escape. Conservative by design: a false positive only costs
/// the multiline code path, never correctness.
pub fn regex_can_match_newline(pattern: &str) -> bool {
    let bytes = pattern.as_bytes();
    let mut i = 0;
    let mut in_class = false;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\\' {
            if i + 1 < bytes.len() {
                match bytes[i + 1] {
                    // \s matches \n; \n is a literal newline. \S excludes
                    // whitespace (incl. \n) so it cannot match it.
                    b's' | b'n' => return true,
                    _ => {}
                }
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if in_class {
            if b == b']' {
                in_class = false;
            }
            i += 1;
            continue;
        }
        match b {
            b'[' => {
                in_class = true;
                // A negated class `[^…]` can match \n unless the class
                // explicitly excludes it — treat any `[^` as multiline.
                if bytes.get(i + 1) == Some(&b'^') {
                    return true;
                }
                i += 1;
            }
            b'(' => {
                // (?s) / (?s:…) / flag groups containing `s`.
                if bytes.get(i + 1) == Some(&b'?') {
                    let mut j = i + 2;
                    let mut saw_s = false;
                    while j < bytes.len() && bytes[j] != b')' && bytes[j] != b':' {
                        if bytes[j] == b's' {
                            saw_s = true;
                        }
                        j += 1;
                    }
                    if saw_s {
                        return true;
                    }
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    false
}

pub fn parse_grep_query(query: &str) -> FfsQuery<'_> {
    let parser = QueryParser::new(GrepConfig);
    parser.parse(query)
}

pub(crate) fn strip_file_path_constraints<'a>(
    constraints: &[Constraint<'a>],
) -> Option<ffs_query_parser::ConstraintVec<'a>> {
    if !constraints
        .iter()
        .any(|c| matches!(c, Constraint::FilePath(_)))
    {
        return None;
    }

    let filtered: ffs_query_parser::ConstraintVec<'a> = constraints
        .iter()
        .filter(|c| !matches!(c, Constraint::FilePath(_)))
        .cloned()
        .collect();

    Some(filtered)
}
