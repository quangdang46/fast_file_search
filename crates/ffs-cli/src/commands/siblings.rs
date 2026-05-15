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

    let definitions = engine.handles.symbols.lookup_exact(&args.name);
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
    let content = std::fs::read_to_string(path).ok()?;
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
// target). For top-level targets, the "parent" is the file itself.
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
    // Otherwise, descend looking for a parent containing the target.
    for parent in outline {
        if let Some(found) = find_in_children(parent, target, target_line) {
            return Some(found);
        }
    }
    None
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
