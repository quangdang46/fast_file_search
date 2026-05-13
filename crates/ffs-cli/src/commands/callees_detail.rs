//! Detailed call-site extraction: shows args, assignment context, and return tracking.

use std::collections::HashSet;

use ffs_symbol::batch::scan_bytes;
use ffs_symbol::types::Lang;

#[derive(Debug, Clone)]
pub struct CallSite {
    pub line: u32,
    pub callee: String,
    pub call_text: String,
    pub args: Vec<String>,
    pub return_var: Option<String>,
    pub is_return: bool,
}

// Language-specific keywords that precede a definition (skip these matches).
fn def_keywords(lang: Lang) -> &'static [&'static str] {
    match lang {
        Lang::Rust => &["fn"],
        Lang::Python => &["def"],
        Lang::Go | Lang::Swift => &["func"],
        Lang::JavaScript | Lang::TypeScript | Lang::Tsx => &["function"],
        Lang::Ruby | Lang::Elixir => &["def"],
        Lang::Java | Lang::Scala | Lang::CSharp | Lang::Kotlin | Lang::C | Lang::Cpp => &[],
        Lang::Php => &["function"],
        _ => &[],
    }
}

fn is_word_boundary(c: Option<char>) -> bool {
    match c {
        None => true,
        Some(c) => !c.is_alphanumeric() && c != '_' && c != '$',
    }
}

fn preceded_by_keyword(before: &str, keywords: &[&'static str]) -> bool {
    let trimmed = before.trim_end();
    for kw in keywords {
        if trimmed == *kw {
            return true;
        }
        if let Some(rest) = trimmed.strip_suffix(kw) {
            if rest
                .chars()
                .last()
                .is_none_or(|c| !c.is_alphanumeric() && c != '_')
            {
                return true;
            }
        }
    }
    false
}

// Pre-filter candidate symbols using aho-corasick over the line range bytes;
// only those symbols are then scanned line-by-line.
fn candidate_symbols<'a>(
    content: &str,
    start_line: u32,
    end_line: u32,
    known_symbols: &'a HashSet<String>,
) -> Vec<&'a str> {
    if known_symbols.is_empty() {
        return Vec::new();
    }
    let mut slice_start = 0usize;
    let mut slice_end = content.len();
    let mut current_line: u32 = 1;
    let mut started = false;
    for (i, c) in content.char_indices() {
        if !started && current_line == start_line {
            slice_start = i;
            started = true;
        }
        if started && current_line == end_line.saturating_add(1) {
            slice_end = i;
            break;
        }
        if c == '\n' {
            current_line += 1;
        }
    }
    let names: Vec<&str> = known_symbols.iter().map(String::as_str).collect();
    let mut hits = scan_bytes(&names, &content.as_bytes()[slice_start..slice_end]);
    hits.sort();
    hits.dedup();
    // Re-bind each hit to the HashSet's own String so the returned slice lives
    // as long as `known_symbols`, not the local `names` Vec.
    hits.into_iter()
        .filter_map(|h| known_symbols.get(h).map(String::as_str))
        .collect()
}

pub fn extract_call_sites(
    content: &str,
    lang: Lang,
    start_line: u32,
    end_line: u32,
    known_symbols: &HashSet<String>,
) -> Vec<CallSite> {
    let mut sites = Vec::new();
    let candidates = candidate_symbols(content, start_line, end_line, known_symbols);
    if candidates.is_empty() {
        return sites;
    }
    let kws = def_keywords(lang);

    for (i, line) in content.lines().enumerate() {
        let lineno = (i + 1) as u32;
        if lineno < start_line || lineno > end_line {
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with('#') {
            continue;
        }
        for sym in &candidates {
            if !line.contains(sym) {
                continue;
            }
            let mut search_from = 0usize;
            while let Some(rel) = line[search_from..].find(sym) {
                let pos = search_from + rel;
                search_from = pos + sym.len();

                // Word boundary: char before must not be ident-continuation.
                let prev_ch = line[..pos].chars().last();
                if !is_word_boundary(prev_ch) {
                    continue;
                }
                let after = &line[pos + sym.len()..];
                // The char after the symbol must also be a word boundary (then
                // optionally followed by `(`).
                let after_first = after.chars().next();
                if !is_word_boundary(after_first) {
                    continue;
                }
                let after_trimmed = after.trim_start();
                if !after_trimmed.starts_with('(') {
                    continue;
                }
                if preceded_by_keyword(&line[..pos], kws) {
                    continue;
                }

                let args = extract_args(after_trimmed);
                let is_return = trimmed.starts_with("return ") || trimmed.starts_with("return(");
                let return_var = extract_assignment_before(line, pos);

                sites.push(CallSite {
                    line: lineno,
                    callee: sym.to_string(),
                    call_text: trimmed.to_string(),
                    args,
                    return_var,
                    is_return,
                });
            }
        }
    }
    sites.sort_by(|a, b| a.line.cmp(&b.line).then_with(|| a.callee.cmp(&b.callee)));
    sites.dedup_by(|a, b| a.line == b.line && a.callee == b.callee);
    sites
}

// Truncate a &str at byte index `cap`, respecting UTF-8 char boundaries.
fn truncate_utf8(s: &str, cap: usize) -> &str {
    if s.len() <= cap {
        return s;
    }
    let mut end = cap;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn extract_args(s: &str) -> Vec<String> {
    // Find the content between the first `(` and its matching `)`.
    let Some(start) = s.find('(') else {
        return Vec::new();
    };
    let mut depth = 0;
    let mut end = None;
    for (i, c) in s[start..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(start + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(end) = end else {
        return Vec::new();
    };
    let inner = &s[start + 1..end];
    if inner.trim().is_empty() {
        return Vec::new();
    }
    inner
        .split(',')
        .map(|a| {
            let t = a.trim();
            if t.len() > 40 {
                format!("{}...", truncate_utf8(t, 37))
            } else {
                t.to_string()
            }
        })
        .collect()
}

fn extract_assignment_before(line: &str, pos: usize) -> Option<String> {
    let before = line[..pos].trim();
    // Patterns: `let x = sym(`, `x = sym(`, `const x = sym(`, `var x = sym(`
    for prefix in &["let ", "const ", "var "] {
        if let Some(rest) = before.strip_prefix(prefix) {
            if let Some(eq_pos) = rest.rfind('=') {
                let name = rest[..eq_pos].trim();
                if !name.is_empty() && name.len() < 60 {
                    return Some(name.to_string());
                }
            }
        }
    }
    // Plain `x = sym(`
    if let Some(eq_pos) = before.rfind('=') {
        let name = before[..eq_pos].trim();
        if !name.is_empty() && !name.contains(' ') && !name.starts_with("//") && name.len() < 60 {
            return Some(name.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn syms(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn extracts_simple_call() {
        let content = "let x = foo(1, 2);\nbar();\n";
        let sites = extract_call_sites(content, Lang::Rust, 1, 2, &syms(&["foo", "bar"]));
        assert_eq!(sites.len(), 2);
        assert_eq!(sites[0].callee, "foo");
        assert_eq!(sites[0].line, 1);
        assert_eq!(sites[0].return_var, Some("x".to_string()));
        assert_eq!(sites[0].args, vec!["1", "2"]);
        assert_eq!(sites[1].callee, "bar");
        assert_eq!(sites[1].line, 2);
    }

    #[test]
    fn detects_return_statement() {
        let content = "return foo(ctx);\n";
        let sites = extract_call_sites(content, Lang::Rust, 1, 1, &syms(&["foo"]));
        assert_eq!(sites.len(), 1);
        assert!(sites[0].is_return);
    }

    #[test]
    fn skips_function_definitions() {
        let content = "fn foo() {}\nlet x = foo();\n";
        let sites = extract_call_sites(content, Lang::Rust, 1, 2, &syms(&["foo"]));
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].line, 2);
    }

    #[test]
    fn extracts_args_compact() {
        let content = "result = func(a, b, c);\n";
        let sites = extract_call_sites(content, Lang::Rust, 1, 1, &syms(&["func"]));
        assert_eq!(sites[0].args, vec!["a", "b", "c"]);
        assert_eq!(sites[0].return_var, Some("result".to_string()));
    }
}
