// Four text renderings of an outline tree:
//
// * agent — header + `[A-B]` left column + bundled imports + footer hint;
//   the dense, agent-friendly default.
// * markdown — nested bullet list, `- kind ` ``name`` ` (start-end)`
// * structured — ASCII tree with ├─ / └─ branch glyphs
// * tabular — fixed-width columns: KIND, NAME, LINES, SIGNATURE
//
// All renderers accept `&[OutlineEntry]` and return a `String` that always
// ends with a trailing newline (or is empty for the legacy renderers).

use ffs_symbol::types::{OutlineEntry, OutlineKind};

pub(crate) struct AgentHeader<'a> {
    pub path: &'a str,
    pub lines: u64,
    pub tokens: u64,
}

pub(crate) fn agent(entries: &[OutlineEntry], header: AgentHeader<'_>) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# {} ({} lines, {} tokens) [outline]\n\n",
        header.path,
        header.lines,
        format_tokens(header.tokens),
    ));

    if entries.is_empty() {
        out.push_str("(no symbols)\n");
        return out;
    }

    let range_width = compute_range_width(entries, 0);
    let imports_taken = render_imports_bundle(entries, &mut out, range_width);
    for entry in &entries[imports_taken..] {
        push_agent_entry(&mut out, entry, 0, range_width);
    }

    out.push_str("\n> Next: drill into a symbol with --section <name> or a line range\n");
    out
}

fn format_tokens(tokens: u64) -> String {
    if tokens >= 1000 {
        let k = (tokens as f64) / 1000.0;
        format!("~{:.1}k", k)
    } else {
        format!("~{}", tokens)
    }
}

fn compute_range_width(entries: &[OutlineEntry], depth: usize) -> usize {
    let mut max = 0;
    for e in entries {
        let w = depth * 2 + range_token(e).len();
        if w > max {
            max = w;
        }
        let inner = compute_range_width(&e.children, depth + 1);
        if inner > max {
            max = inner;
        }
    }
    max
}

fn range_token(e: &OutlineEntry) -> String {
    format!("[{}-{}]", e.start_line, e.end_line)
}

fn push_agent_entry(out: &mut String, e: &OutlineEntry, depth: usize, range_width: usize) {
    let indent = "  ".repeat(depth);
    let range = range_token(e);
    let visible = indent.len() + range.len();
    let pad = " ".repeat(range_width.saturating_sub(visible) + 3);

    out.push_str(&indent);
    out.push_str(&range);
    out.push_str(&pad);
    out.push_str(kind_label(e.kind));
    out.push(' ');
    out.push_str(&e.name);
    out.push('\n');

    if let Some(sig) = e.signature.as_ref() {
        let trimmed = sig.trim();
        let head = format!("{} {}", kind_label(e.kind), e.name);
        if !trimmed.is_empty() && trimmed != head {
            let col = range_width + 3;
            out.push_str(&" ".repeat(col));
            out.push_str(trimmed);
            out.push('\n');
        }
    }

    for c in &e.children {
        push_agent_entry(out, c, depth + 1, range_width);
    }
}

// Bundle leading consecutive imports into a single line: `[A-B] imports: a, b(2), c`.
// Returns the number of entries consumed.
fn render_imports_bundle(entries: &[OutlineEntry], out: &mut String, range_width: usize) -> usize {
    let count = entries
        .iter()
        .take_while(|e| matches!(e.kind, OutlineKind::Import))
        .count();
    if count == 0 {
        return 0;
    }

    let first = &entries[0];
    let last = &entries[count - 1];
    let mut counts: Vec<(String, usize)> = Vec::new();
    for e in &entries[..count] {
        let src = parse_import_source(e.signature.as_deref().unwrap_or(""))
            .unwrap_or_else(|| e.name.clone());
        if let Some((_, c)) = counts.iter_mut().find(|(k, _)| k == &src) {
            *c += 1;
        } else {
            counts.push((src, 1));
        }
    }
    let parts: Vec<String> = counts
        .into_iter()
        .map(|(s, c)| if c > 1 { format!("{s}({c})") } else { s })
        .collect();

    let range = format!("[{}-{}]", first.start_line, last.end_line);
    let pad = " ".repeat(range_width.saturating_sub(range.len()) + 3);
    out.push_str(&range);
    out.push_str(&pad);
    out.push_str("imports: ");
    out.push_str(&parts.join(", "));
    out.push('\n');
    count
}

// Extract the imported module name from an import statement's first line.
// Handles JS/TS (from / require), Python, Rust (`use a::b::c;`), and bare
// quoted forms. Returns None when the shape isn't recognised.
fn parse_import_source(sig: &str) -> Option<String> {
    let s = sig.trim();
    if s.is_empty() {
        return None;
    }

    // Quoted module name (most JS/TS, ES `import 'x'`, CJS `require('x')`).
    for q in ['"', '\''] {
        if let Some(start) = s.find(q) {
            if let Some(end_rel) = s[start + 1..].find(q) {
                let inner = &s[start + 1..start + 1 + end_rel];
                if !inner.is_empty() {
                    return Some(inner.to_string());
                }
            }
        }
    }

    // Python `from X import ...`
    if let Some(rest) = s.strip_prefix("from ") {
        if let Some(token) = rest.split_whitespace().next() {
            return Some(token.to_string());
        }
    }

    // Python `import X`, Java `import X.Y;`
    if let Some(rest) = s.strip_prefix("import ") {
        if let Some(token) = rest.split_whitespace().next() {
            return Some(token.trim_end_matches(';').to_string());
        }
    }

    // Rust `use a::b::c;` — group by crate root.
    if let Some(rest) = s.strip_prefix("use ") {
        let head = rest.split(['{', ';', ' ']).next().unwrap_or(rest);
        let root = head.split("::").next().unwrap_or(head);
        if !root.is_empty() {
            return Some(root.to_string());
        }
    }

    None
}

pub(crate) fn markdown(entries: &[OutlineEntry]) -> String {
    let mut out = String::new();
    for e in entries {
        push_markdown(&mut out, e, 0);
    }
    out
}

fn push_markdown(out: &mut String, e: &OutlineEntry, depth: usize) {
    let indent = "  ".repeat(depth);
    out.push_str(&indent);
    out.push_str("- ");
    out.push_str(kind_label(e.kind));
    out.push(' ');
    out.push('`');
    out.push_str(&e.name);
    out.push('`');
    out.push_str(&format!(" ({}-{})\n", e.start_line, e.end_line));
    if let Some(sig) = e.signature.as_ref() {
        if !sig.is_empty() {
            out.push_str(&"  ".repeat(depth + 1));
            out.push_str("- `");
            out.push_str(sig);
            out.push_str("`\n");
        }
    }
    for c in &e.children {
        push_markdown(out, c, depth + 1);
    }
}

pub(crate) fn structured(entries: &[OutlineEntry]) -> String {
    let mut out = String::new();
    let n = entries.len();
    for (i, e) in entries.iter().enumerate() {
        push_structured(&mut out, e, "", i + 1 == n);
    }
    out
}

fn push_structured(out: &mut String, e: &OutlineEntry, prefix: &str, is_last: bool) {
    let branch = if is_last { "└─ " } else { "├─ " };
    out.push_str(prefix);
    out.push_str(branch);
    out.push_str(kind_label(e.kind));
    out.push_str(": ");
    out.push_str(&e.name);
    out.push_str(&format!(" ({}-{})\n", e.start_line, e.end_line));

    let child_prefix = format!("{prefix}{}", if is_last { "   " } else { "│  " });
    let n = e.children.len();
    for (i, c) in e.children.iter().enumerate() {
        push_structured(out, c, &child_prefix, i + 1 == n);
    }
}

pub(crate) fn tabular(entries: &[OutlineEntry]) -> String {
    let mut rows: Vec<(String, String, String, String)> = Vec::new();
    for e in entries {
        push_tabular_rows(e, 0, &mut rows);
    }
    if rows.is_empty() {
        return String::new();
    }
    // Compute column widths (cap NAME so very long identifiers don't blow up
    // the layout; signatures are emitted as-is, last column).
    let header = (
        "KIND".to_string(),
        "NAME".to_string(),
        "LINES".to_string(),
        "SIGNATURE".to_string(),
    );
    let mut all = vec![header];
    all.extend(rows);
    let kw = all.iter().map(|r| r.0.len()).max().unwrap_or(4);
    let nw = all.iter().map(|r| r.1.len()).max().unwrap_or(4).min(40);
    let lw = all.iter().map(|r| r.2.len()).max().unwrap_or(5);

    let mut out = String::new();
    for (k, n, l, s) in &all {
        out.push_str(&format!(
            "{:<kw$}  {:<nw$}  {:<lw$}  {}\n",
            k,
            truncate(n, nw),
            l,
            s,
            kw = kw,
            nw = nw,
            lw = lw,
        ));
    }
    out
}

fn push_tabular_rows(
    e: &OutlineEntry,
    depth: usize,
    out: &mut Vec<(String, String, String, String)>,
) {
    let kind = kind_label(e.kind).to_string();
    let name = if depth == 0 {
        e.name.clone()
    } else {
        format!("{}{}", "  ".repeat(depth), e.name)
    };
    let lines = format!("{}-{}", e.start_line, e.end_line);
    let sig = e.signature.clone().unwrap_or_default();
    out.push((kind, name, lines, sig));
    for c in &e.children {
        push_tabular_rows(c, depth + 1, out);
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut taken: String = s.chars().take(max.saturating_sub(1)).collect();
    taken.push('…');
    taken
}

fn kind_label(k: OutlineKind) -> &'static str {
    match k {
        OutlineKind::Import => "import",
        OutlineKind::Function => "function",
        OutlineKind::Class => "class",
        OutlineKind::Struct => "struct",
        OutlineKind::Interface => "interface",
        OutlineKind::TypeAlias => "type",
        OutlineKind::Enum => "enum",
        OutlineKind::Constant => "const",
        OutlineKind::Variable => "var",
        OutlineKind::ImmutableVariable => "let",
        OutlineKind::Export => "export",
        OutlineKind::Property => "property",
        OutlineKind::Module => "module",
        OutlineKind::TestSuite => "test_suite",
        OutlineKind::TestCase => "test",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        agent, kind_label, markdown, parse_import_source, structured, tabular, AgentHeader,
    };
    use ffs_symbol::types::{OutlineEntry, OutlineKind};

    fn entry(kind: OutlineKind, name: &str, start: u32, end: u32) -> OutlineEntry {
        OutlineEntry {
            kind,
            name: name.to_string(),
            start_line: start,
            end_line: end,
            signature: None,
            children: Vec::new(),
            doc: None,
        }
    }

    fn entry_with_sig(
        kind: OutlineKind,
        name: &str,
        start: u32,
        end: u32,
        sig: &str,
    ) -> OutlineEntry {
        let mut e = entry(kind, name, start, end);
        e.signature = Some(sig.to_string());
        e
    }

    #[test]
    fn markdown_empty_input_is_empty_string() {
        let out = markdown(&[]);
        assert_eq!(out, "");
    }

    #[test]
    fn markdown_renders_top_level_entries() {
        let entries = vec![
            entry(OutlineKind::Function, "main", 12, 50),
            entry(OutlineKind::Struct, "Config", 5, 10),
        ];
        let out = markdown(&entries);
        assert_eq!(out, "- function `main` (12-50)\n- struct `Config` (5-10)\n");
    }

    #[test]
    fn markdown_indents_children_and_includes_signature() {
        let mut parent = entry_with_sig(OutlineKind::Function, "outer", 1, 20, "fn outer()");
        parent
            .children
            .push(entry(OutlineKind::Function, "inner", 5, 10));
        let out = markdown(&[parent]);
        assert_eq!(
            out,
            "- function `outer` (1-20)\n  - `fn outer()`\n  - function `inner` (5-10)\n"
        );
    }

    #[test]
    fn structured_uses_branches_and_marks_last_child() {
        let mut parent = entry(OutlineKind::Class, "Foo", 1, 30);
        parent
            .children
            .push(entry(OutlineKind::Function, "a", 2, 5));
        parent
            .children
            .push(entry(OutlineKind::Function, "b", 6, 10));
        let other = entry(OutlineKind::Struct, "Bar", 35, 40);
        let out = structured(&[parent, other]);
        let expected = "\
├─ class: Foo (1-30)\n\
│  ├─ function: a (2-5)\n\
│  └─ function: b (6-10)\n\
└─ struct: Bar (35-40)\n";
        assert_eq!(out, expected);
    }

    #[test]
    fn structured_empty_input_is_empty_string() {
        let out = structured(&[]);
        assert_eq!(out, "");
    }

    #[test]
    fn tabular_renders_header_and_padded_columns() {
        let entries = vec![
            entry_with_sig(OutlineKind::Function, "main", 12, 50, "fn main()"),
            entry(OutlineKind::Struct, "Config", 5, 10),
        ];
        let out = tabular(&entries);
        // Header row must be present and aligned with the data rows.
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("KIND"));
        assert!(lines[0].contains("NAME"));
        assert!(lines[0].contains("LINES"));
        assert!(lines[0].contains("SIGNATURE"));
        assert!(lines[1].contains("function"));
        assert!(lines[1].contains("main"));
        assert!(lines[1].contains("12-50"));
        assert!(lines[1].contains("fn main()"));
        assert!(lines[2].contains("struct"));
        assert!(lines[2].contains("Config"));
        assert!(lines[2].contains("5-10"));
    }

    #[test]
    fn tabular_empty_input_is_empty_string() {
        assert_eq!(tabular(&[]), "");
    }

    #[test]
    fn tabular_indents_child_names_so_hierarchy_is_visible() {
        let mut parent = entry(OutlineKind::Class, "Cls", 1, 20);
        parent
            .children
            .push(entry(OutlineKind::Property, "field", 2, 2));
        let out = tabular(&[parent]);
        // Child name should be prefixed with two spaces of indent.
        assert!(out.contains("  field"));
    }

    #[test]
    fn agent_header_renders_path_lines_and_tokens() {
        let entries = vec![entry(OutlineKind::Function, "main", 1, 5)];
        let out = agent(
            &entries,
            AgentHeader {
                path: "src/foo.rs",
                lines: 5,
                tokens: 42,
            },
        );
        assert!(out.starts_with("# src/foo.rs (5 lines, ~42 tokens) [outline]\n\n"));
        assert!(out.contains("function main"));
        assert!(out.trim_end().ends_with("--section <name> or a line range"));
    }

    #[test]
    fn agent_renders_kilo_token_count_with_one_decimal() {
        let entries = vec![entry(OutlineKind::Function, "f", 1, 1)];
        let out = agent(
            &entries,
            AgentHeader {
                path: "src/big.ts",
                lines: 258,
                tokens: 3_400,
            },
        );
        assert!(out.starts_with("# src/big.ts (258 lines, ~3.4k tokens) [outline]"));
    }

    #[test]
    fn agent_bundles_consecutive_imports_into_one_line() {
        let mut imp1 = entry(OutlineKind::Import, "express", 1, 1);
        imp1.signature = Some("import express from 'express';".into());
        let mut imp2 = entry(OutlineKind::Import, "jwt", 2, 2);
        imp2.signature = Some("import jwt from 'jsonwebtoken';".into());
        let mut imp3 = entry(OutlineKind::Import, "router", 3, 3);
        imp3.signature = Some("const Router = require('express');".into());
        let func = entry(OutlineKind::Function, "handle", 5, 10);
        let out = agent(
            &[imp1, imp2, imp3, func],
            AgentHeader {
                path: "app.ts",
                lines: 10,
                tokens: 80,
            },
        );
        // Imports collapsed; `express` deduped to count 2.
        assert!(
            out.contains("imports: express(2), jsonwebtoken"),
            "output:\n{out}"
        );
        // Range covers the whole import block.
        assert!(out.contains("[1-3]"));
        // Function still rendered after the bundle.
        assert!(out.contains("function handle"));
    }

    #[test]
    fn agent_emits_signature_on_indented_continuation_line() {
        let func = entry_with_sig(
            OutlineKind::Function,
            "validateToken",
            24,
            42,
            "function validateToken(token: string): Claims | null",
        );
        let out = agent(
            &[func],
            AgentHeader {
                path: "src/auth.ts",
                lines: 100,
                tokens: 600,
            },
        );
        assert!(out.contains("function validateToken(token: string): Claims | null"));
    }

    #[test]
    fn agent_indents_children_two_spaces_per_depth() {
        let mut parent = entry(OutlineKind::Class, "Mgr", 1, 50);
        parent
            .children
            .push(entry(OutlineKind::Function, "a", 5, 10));
        parent
            .children
            .push(entry(OutlineKind::Function, "b", 11, 20));
        let out = agent(
            &[parent],
            AgentHeader {
                path: "x.rs",
                lines: 50,
                tokens: 200,
            },
        );
        // Children rendered with two-space indent prefix.
        assert!(out
            .lines()
            .any(|l| l.starts_with("  [") && l.contains("function a")));
        assert!(out
            .lines()
            .any(|l| l.starts_with("  [") && l.contains("function b")));
    }

    #[test]
    fn agent_handles_empty_outline_with_no_symbols_marker() {
        let out = agent(
            &[],
            AgentHeader {
                path: "empty.rs",
                lines: 0,
                tokens: 0,
            },
        );
        assert!(out.contains("(no symbols)"));
        // No footer hint when there are no symbols to drill into.
        assert!(!out.contains("> Next:"));
    }

    #[test]
    fn parse_import_source_extracts_quoted_module() {
        assert_eq!(
            parse_import_source("import x from 'express';"),
            Some("express".into())
        );
        assert_eq!(
            parse_import_source("const a = require(\"react\");"),
            Some("react".into())
        );
    }

    #[test]
    fn parse_import_source_handles_python_from_and_import() {
        assert_eq!(
            parse_import_source("from collections import OrderedDict"),
            Some("collections".into())
        );
        assert_eq!(parse_import_source("import os"), Some("os".into()));
    }

    #[test]
    fn parse_import_source_groups_rust_use_by_crate_root() {
        assert_eq!(
            parse_import_source("use anyhow::Result;"),
            Some("anyhow".into())
        );
        assert_eq!(
            parse_import_source("use std::collections::HashMap;"),
            Some("std".into())
        );
    }

    #[test]
    fn kind_label_covers_every_variant() {
        // Sanity: just call it on every variant; the test is that the match
        // is exhaustive (compile-time) and that no label is empty.
        let all = [
            OutlineKind::Import,
            OutlineKind::Function,
            OutlineKind::Class,
            OutlineKind::Struct,
            OutlineKind::Interface,
            OutlineKind::TypeAlias,
            OutlineKind::Enum,
            OutlineKind::Constant,
            OutlineKind::Variable,
            OutlineKind::ImmutableVariable,
            OutlineKind::Export,
            OutlineKind::Property,
            OutlineKind::Module,
            OutlineKind::TestSuite,
            OutlineKind::TestCase,
        ];
        for k in all {
            assert!(!kind_label(k).is_empty());
        }
    }
}
