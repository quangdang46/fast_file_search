// Three text renderings of an outline tree:
//
// * markdown — nested bullet list, `- kind ` ``name`` ` (start-end)`
// * structured — ASCII tree with ├─ / └─ branch glyphs
// * tabular — fixed-width columns: KIND, NAME, LINES, SIGNATURE
//
// All three accept the same `&[OutlineEntry]` and return a `String` that
// always ends with a trailing newline (or is empty).

use fff_symbol::types::{OutlineEntry, OutlineKind};

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
    use super::{kind_label, markdown, structured, tabular};
    use fff_symbol::types::{OutlineEntry, OutlineKind};

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
