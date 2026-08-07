use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Result;
use clap::Parser;
use serde::Serialize;

use ffs_engine::Engine;
use ffs_symbol::lang::detect_file_type;
use ffs_symbol::types::{FileType, OutlineEntry, OutlineKind};

use crate::cli::OutputFormat;
use crate::commands::pagination::{footer, Page};

#[derive(Debug, Parser)]
pub struct Args {
    /// Symbol whose siblings (same parent scope) should be listed.
    pub name: String,

    /// Maximum siblings returned in this page.
    #[arg(long, default_value_t = 100)]
    pub limit: usize,

    /// Skip this many siblings before starting the page.
    #[arg(long, default_value_t = 0)]
    pub offset: usize,

    /// Include `Import` entries as siblings. Off by default because most
    /// callers want structural peers (functions/classes/structs), not the
    /// import block.
    #[arg(long, default_value_t = false)]
    pub include_imports: bool,
}

#[derive(Debug, Serialize)]
struct SiblingHit {
    name: String,
    kind: String,
    path: String,
    line: u32,
    end_line: u32,
    // Path to the parent definition the target lives inside, or "<file>" when
    // the target is a top-level entry. Useful when the same target name has
    // multiple definitions in the same file (rare but possible).
    parent: String,
    // The definition site the sibling is reported for. A target with N
    // definitions produces up to N sibling groups, all flattened into the
    // same Vec but keyed by `target_path` so callers can group them back.
    target_path: String,
    target_line: u32,
}

#[derive(Debug, Serialize)]
struct SiblingsOutput {
    name: String,
    hits: Vec<SiblingHit>,
    total: usize,
    offset: usize,
    has_more: bool,
}

pub fn run(args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    let engine = crate::cache::load_or_build_engine(root);

    let mut definitions = engine.handles.symbols.lookup_exact(&args.name);
    // The symbol index is built with par_iter(), so per-name definition order
    // is arbitrary. Sort by (path, line) so scope resolution is deterministic
    // regardless of parallel insertion order.
    definitions.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then_with(|| a.line.cmp(&b.line))
            .then_with(|| a.end_line.cmp(&b.end_line))
    });
    let mut hits: Vec<SiblingHit> = Vec::new();

    for def in &definitions {
        let Some(outline) = load_outline(&engine, &def.path) else {
            continue;
        };
        let Some((parent_label, siblings)) = find_siblings(&outline, &args.name, def.line) else {
            continue;
        };
        let target_path = def.path.to_string_lossy().to_string();
        for s in siblings {
            if !args.include_imports && s.kind == OutlineKind::Import {
                continue;
            }
            hits.push(SiblingHit {
                name: s.name.clone(),
                kind: format!("{:?}", s.kind).to_lowercase(),
                path: target_path.clone(),
                line: s.start_line,
                end_line: s.end_line,
                parent: parent_label.clone(),
                target_path: target_path.clone(),
                target_line: def.line,
            });
        }
    }

    // Bug 8: dedup by (name, path, line). When a symbol has multiple
    // definition sites (e.g. an `export function` plus a redeclared `function`
    // on the same line in TS), the same peers are otherwise emitted once per
    // definition. Keep the first occurrence so pagination counts agree with
    // visible rows.
    let mut seen: std::collections::HashSet<(String, String, u32)> =
        std::collections::HashSet::new();
    hits.retain(|h| seen.insert((h.name.clone(), h.path.clone(), h.line)));

    let page = Page::paginate(hits, args.offset, args.limit);
    let payload = SiblingsOutput {
        name: args.name,
        total: page.total,
        offset: page.offset,
        has_more: page.has_more,
        hits: page.items,
    };
    super::emit(format, &payload, |p| {
        let mut out = String::new();
        for h in &p.hits {
            out.push_str(&format!(
                "{} ({}) @ {}:{}  [parent: {}, target: {}:{}]\n",
                h.name, h.kind, h.path, h.line, h.parent, h.target_path, h.target_line
            ));
        }
        if p.total == 0 {
            out.push_str("[no siblings found]\n");
        } else {
            out.push_str(&footer(p.total, p.offset, p.hits.len(), p.has_more));
        }
        out
    })
}

fn load_outline(engine: &Engine, path: &PathBuf) -> Option<Vec<OutlineEntry>> {
    let lang = match detect_file_type(path) {
        FileType::Code(l) => l,
        _ => return None,
    };
    let content = ffs_search::bom::read_file(path).ok()?;
    let mtime = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    Some(
        engine
            .handles
            .outlines
            .get_or_compute(path, mtime, &content, lang),
    )
}

// Walk the outline looking for an entry whose name == target and whose
// start_line == target_line. Return its parent's siblings (i.e. peers of the
// target). For top-level targets, the "parent" is the file itself. For a Rust
// method, the "parent" is its `impl Type` block, so peers are the other
// methods of the same impl.
fn find_siblings(
    outline: &[OutlineEntry],
    target: &str,
    target_line: u32,
) -> Option<(String, Vec<OutlineEntry>)> {
    // First, see if the target is a top-level entry.
    if outline
        .iter()
        .any(|e| e.name == target && e.start_line == target_line)
    {
        let peers: Vec<OutlineEntry> = outline
            .iter()
            .filter(|e| !(e.name == target && e.start_line == target_line))
            .cloned()
            .collect();
        return Some(("<file>".to_string(), peers));
    }
    // Rust: the target may be a method inside an `impl` block. The impl
    // container's name is the impl'd type, so match either the container name
    // or a `Type` / `Trait for Type` form (e.g. `impl Foo`, `impl Debug for
    // Foo`).
    for parent in outline {
        if parent.kind == OutlineKind::Impl
            && impl_contains_target(parent, target, target_line)
            && impl_named(parent, target)
        {
            let peers: Vec<OutlineEntry> = parent
                .children
                .iter()
                .filter(|c| !(c.name == target && c.start_line == target_line))
                .cloned()
                .collect();
            return Some((parent.name.clone(), peers));
        }
    }
    // Otherwise, descend looking for a parent containing the target.
    for parent in outline {
        if let Some(found) = find_in_children(parent, target, target_line) {
            return Some(found);
        }
    }
    None
}

fn impl_contains_target(impl_: &OutlineEntry, target: &str, target_line: u32) -> bool {
    impl_
        .children
        .iter()
        .any(|c| c.name == target && c.start_line == target_line)
}

// True when the impl block's name (its impl'd type) matches the target. This
// resolves the ambiguity where the target name is a top-level definition too:
// the impl must be a scope for that exact name, not just a container that
// happens to hold the line.
fn impl_named(impl_: &OutlineEntry, target: &str) -> bool {
    impl_.name == target
        || impl_
            .name
            .strip_prefix(target)
            .is_some_and(|rest| rest.starts_with(" for "))
}

fn find_in_children(
    parent: &OutlineEntry,
    target: &str,
    target_line: u32,
) -> Option<(String, Vec<OutlineEntry>)> {
    if parent
        .children
        .iter()
        .any(|c| c.name == target && c.start_line == target_line)
    {
        let peers: Vec<OutlineEntry> = parent
            .children
            .iter()
            .filter(|c| !(c.name == target && c.start_line == target_line))
            .cloned()
            .collect();
        return Some((parent.name.clone(), peers));
    }
    for c in &parent.children {
        if let Some(found) = find_in_children(c, target, target_line) {
            return Some(found);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{find_siblings, OutlineEntry};
    use ffs_symbol::types::OutlineKind;

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

    #[test]
    fn top_level_target_returns_other_top_level_entries() {
        let outline = vec![
            entry(OutlineKind::Function, "alpha", 1, 5),
            entry(OutlineKind::Function, "beta", 7, 12),
            entry(OutlineKind::Struct, "Config", 14, 20),
        ];
        let (parent, peers) = find_siblings(&outline, "beta", 7).expect("beta found");
        assert_eq!(parent, "<file>");
        let names: Vec<&str> = peers.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "Config"]);
    }

    #[test]
    fn nested_target_returns_other_children_under_same_parent() {
        let mut cls = entry(OutlineKind::Class, "Cls", 1, 50);
        cls.children.push(entry(OutlineKind::Function, "a", 5, 10));
        cls.children.push(entry(OutlineKind::Function, "b", 12, 18));
        cls.children.push(entry(OutlineKind::Function, "c", 20, 25));
        let other = entry(OutlineKind::Function, "free", 60, 65);
        let outline = vec![cls, other];

        let (parent, peers) = find_siblings(&outline, "b", 12).expect("b found");
        assert_eq!(parent, "Cls");
        let names: Vec<&str> = peers.iter().map(|e| e.name.as_str()).collect();
        // `free` is NOT a sibling of `b` because they're in different scopes.
        assert_eq!(names, vec!["a", "c"]);
    }

    #[test]
    fn unknown_target_returns_none() {
        let outline = vec![entry(OutlineKind::Function, "alpha", 1, 5)];
        assert!(find_siblings(&outline, "nope", 1).is_none());
    }

    #[test]
    fn deeply_nested_target_found_via_recursion() {
        let mut leaf = entry(OutlineKind::Function, "deep", 30, 40);
        leaf.children
            .push(entry(OutlineKind::Function, "deeper", 32, 35));
        let mut mid = entry(OutlineKind::Class, "Mid", 20, 50);
        mid.children.push(leaf);
        mid.children
            .push(entry(OutlineKind::Function, "midpeer", 41, 45));
        let outline = vec![mid];

        let (parent, peers) = find_siblings(&outline, "deeper", 32).expect("deeper found");
        assert_eq!(parent, "deep");
        assert!(peers.is_empty());
    }

    #[test]
    fn rust_method_resolves_to_impl_peers_not_top_level() {
        // `add` exists both as a top-level function and as an impl method;
        // the impl method's peers are the impl's other methods.
        let mut impl_ = entry(OutlineKind::Impl, "UnifiedScanner", 3, 30);
        impl_
            .children
            .push(entry(OutlineKind::Function, "new", 4, 10));
        impl_
            .children
            .push(entry(OutlineKind::Function, "add", 12, 20));
        let top = entry(OutlineKind::Function, "add", 1, 2);
        let outline = vec![top, impl_];

        let (parent, peers) = find_siblings(&outline, "add", 12).expect("add@12 found");
        assert_eq!(parent, "UnifiedScanner");
        let names: Vec<&str> = peers.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["new"]);
    }

    #[test]
    fn rust_method_resolves_to_trait_impl_peers() {
        let mut impl_ = entry(OutlineKind::Impl, "Display for Foo", 3, 30);
        impl_
            .children
            .push(entry(OutlineKind::Function, "fmt", 4, 20));
        let outline = vec![impl_];

        let (parent, peers) = find_siblings(&outline, "fmt", 4).expect("fmt@4 found");
        assert_eq!(parent, "Display for Foo");
        assert!(peers.is_empty());
    }

    #[test]
    fn top_level_target_does_not_leak_into_impl_scope() {
        // The same name exists at top level (line 1) and as an impl method
        // (line 12); resolving the top-level def must stay at file scope.
        let mut impl_ = entry(OutlineKind::Impl, "UnifiedScanner", 3, 30);
        impl_
            .children
            .push(entry(OutlineKind::Function, "new", 4, 10));
        impl_
            .children
            .push(entry(OutlineKind::Function, "add", 12, 20));
        let top = entry(OutlineKind::Function, "add", 1, 2);
        let outline = vec![top, impl_];

        let (parent, peers) = find_siblings(&outline, "add", 1).expect("add@1 found");
        assert_eq!(parent, "<file>");
        let names: Vec<&str> = peers.iter().map(|e| e.name.as_str()).collect();
        assert!(!names.contains(&"new"));
    }

    #[test]
    fn target_with_same_name_as_peer_at_different_line_is_not_self() {
        // Pathological: two top-level entries share the name (overload-ish).
        // The one we asked for (line 7) is the "self"; the other (line 20)
        // remains as a sibling.
        let outline = vec![
            entry(OutlineKind::Function, "f", 7, 10),
            entry(OutlineKind::Function, "f", 20, 25),
        ];
        let (parent, peers) = find_siblings(&outline, "f", 7).expect("f@7 found");
        assert_eq!(parent, "<file>");
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].start_line, 20);
    }
}
