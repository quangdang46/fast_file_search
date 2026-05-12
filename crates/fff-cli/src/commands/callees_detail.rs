//! Detailed call-site extraction: shows args, assignment context, and return tracking.

use std::collections::HashSet;

use fff_symbol::types::Lang;

/// One call site inside a function body.
#[derive(Debug, Clone)]
pub struct CallSite {
    pub line: u32,
    pub callee: String,
    pub call_text: String,
    pub args: Vec<String>,
    pub return_var: Option<String>,
    pub is_return: bool,
}

/// Extract detailed call-site info from source lines. Only includes calls to
/// symbols in `known_symbols`. Operates on text matching (no AST).
pub fn extract_call_sites(
    content: &str,
    _lang: Lang,
    start_line: u32,
    end_line: u32,
    known_symbols: &HashSet<String>,
) -> Vec<CallSite> {
    let mut sites = Vec::new();
    let start = start_line.saturating_sub(1) as usize;
    let end = end_line as usize;

    for (i, line) in content.lines().enumerate() {
        let lineno = (i + 1) as u32;
        if lineno < start_line || lineno > end_line {
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with('#') {
            continue;
        }
        for sym in known_symbols {
            if !line.contains(sym.as_str()) {
                continue;
            }
            // Check for call pattern: `sym(` or `sym (` (but not `fn sym`)
            if let Some(pos) = line.find(sym.as_str()) {
                let after = &line[pos + sym.len()..];
                let after_trimmed = after.trim_start();
                if !after_trimmed.starts_with('(') {
                    continue;
                }
                // Skip if preceded by fn/def/func/function
                let before = &line[..pos].trim_end();
                if before.ends_with("fn") || before.ends_with("def") || before.ends_with("func") || before.ends_with("function") {
                    continue;
                }

                let args = extract_args(after_trimmed);
                let is_return = trimmed.starts_with("return ") || trimmed.starts_with("return(");
                let return_var = extract_assignment_before(line, pos);

                sites.push(CallSite {
                    line: lineno,
                    callee: sym.clone(),
                    call_text: trimmed.to_string(),
                    args,
                    return_var,
                    is_return,
                });
            }
        }
    }
    // Dedup by (line, callee).
    sites.sort_by(|a, b| a.line.cmp(&b.line).then_with(|| a.callee.cmp(&b.callee)));
    sites.dedup_by(|a, b| a.line == b.line && a.callee == b.callee);
    sites
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
                format!("{}...", &t[..37])
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
        if !name.is_empty()
            && !name.contains(' ')
            && !name.starts_with("//")
            && name.len() < 60
        {
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
