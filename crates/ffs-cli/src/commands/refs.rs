// `ffs refs <name>` — definitions + single-hop usages in one shot.
// Definitions come from the symbol index (full list, no pagination).
// Usages reuse the `callers --hops 1` text-confirm pass; pagination
// applies to usages only so the defs block is always complete.

use std::path::Path;
use std::time::SystemTime;

use anyhow::Result;
use clap::Parser;
use serde::Serialize;

use ffs_engine::{Engine, PreFilterStack};
use ffs_symbol::lang::detect_file_type;
use ffs_symbol::types::FileType;

use crate::cli::OutputFormat;
use crate::commands::callers_bfs::{enclosing_symbol, CandidateFile};
use crate::commands::pagination::{footer, Page};

#[derive(Debug, Parser)]
pub struct Args {
    /// Symbol name to find references (definitions + usages) for.
    pub name: String,

    /// Maximum usages returned in this page. Definitions ignore this.
    #[arg(long, default_value_t = 100)]
    pub limit: usize,

    /// Skip this many usages before starting the page. Definitions ignore this.
    #[arg(long, default_value_t = 0)]
    pub offset: usize,
}

#[derive(Debug, Serialize)]
struct RefDefinition {
    path: String,
    line: u32,
    end_line: u32,
    kind: String,
    weight: u16,
}

#[derive(Debug, Serialize)]
struct RefUsage {
    path: String,
    line: u32,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    enclosing: Option<String>,
}

#[derive(Debug, Serialize)]
struct RefsOutput {
    name: String,
    definitions: Vec<RefDefinition>,
    usages: Vec<RefUsage>,
    total_usages: usize,
    offset: usize,
    has_more: bool,
}

pub fn run(args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    let engine = crate::cache::load_or_build_engine(root);

    let definitions = collect_definitions(&engine, &args.name);
    let candidates = build_candidates(root);
    let usages = collect_usages(&engine, &args.name, &candidates);

    let page = Page::paginate(usages, args.offset, args.limit);
    let payload = RefsOutput {
        name: args.name.clone(),
        definitions,
        total_usages: page.total,
        offset: page.offset,
        has_more: page.has_more,
        usages: page.items,
    };
    super::emit(format, &payload, render_text)
}

fn collect_definitions(engine: &Engine, name: &str) -> Vec<RefDefinition> {
    engine
        .handles
        .symbols
        .lookup_exact(name)
        .into_iter()
        .map(|loc| RefDefinition {
            path: loc.path.to_string_lossy().to_string(),
            line: loc.line,
            end_line: loc.end_line,
            kind: loc.kind,
            weight: loc.weight,
        })
        .collect()
}

fn build_candidates(root: &Path) -> Vec<CandidateFile> {
    super::walk_files(root)
        .into_iter()
        .filter_map(|path| {
            let meta = std::fs::metadata(&path).ok()?;
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let content = std::fs::read_to_string(&path).ok()?;
            let lang = match detect_file_type(&path) {
                FileType::Code(l) => Some(l),
                _ => None,
            };
            Some(CandidateFile {
                path,
                mtime,
                content,
                lang,
            })
        })
        .collect()
}

fn collect_usages(engine: &Engine, name: &str, candidates: &[CandidateFile]) -> Vec<RefUsage> {
    let definitions = engine.handles.symbols.lookup_exact(name);
    let definition_paths: Vec<String> = definitions
        .iter()
        .map(|d| d.path.to_string_lossy().to_string())
        .collect();

    let stack = PreFilterStack::new(engine.handles.bloom.clone());
    let confirm_input: Vec<(std::path::PathBuf, SystemTime, String)> = candidates
        .iter()
        .map(|c| (c.path.clone(), c.mtime, c.content.clone()))
        .collect();
    let survivors = stack.confirm_symbol(&confirm_input, name);
    let survivor_set: std::collections::HashSet<&std::path::Path> =
        survivors.iter().map(|s| s.path.as_path()).collect();

    let mut usages: Vec<RefUsage> = Vec::new();
    for cf in candidates {
        if !survivor_set.contains(cf.path.as_path()) {
            continue;
        }
        let path_str = cf.path.to_string_lossy().to_string();
        let in_defn_file = definition_paths.contains(&path_str);
        let definition_lines: Vec<u32> = if in_defn_file {
            definitions
                .iter()
                .filter(|d| d.path.to_string_lossy() == path_str)
                .map(|d| d.line)
                .collect()
        } else {
            Vec::new()
        };

        let outline = if let Some(lang) = cf.lang {
            engine
                .handles
                .outlines
                .get_or_compute(&cf.path, cf.mtime, &cf.content, lang)
        } else {
            Vec::new()
        };

        for (lineno, line) in cf.content.lines().enumerate() {
            let lineno = (lineno + 1) as u32;
            if !line.contains(name) {
                continue;
            }
            if definition_lines.contains(&lineno) {
                continue;
            }
            usages.push(RefUsage {
                path: path_str.clone(),
                line: lineno,
                text: line.to_string(),
                enclosing: enclosing_symbol(&outline, lineno),
            });
        }
    }
    usages
}

fn render_text(p: &RefsOutput) -> String {
    let mut out = String::new();
    out.push_str(&format!("Definitions ({}):\n", p.definitions.len()));
    if p.definitions.is_empty() {
        out.push_str("  [none]\n");
    } else {
        for d in &p.definitions {
            out.push_str(&format!(
                "  {}:{} ({}, w={})\n",
                d.path, d.line, d.kind, d.weight
            ));
        }
    }

    out.push_str(&format!("\nUsages ({}):\n", p.total_usages));
    if p.total_usages == 0 {
        out.push_str("  [none]\n");
    } else {
        for u in &p.usages {
            let encl = u.enclosing.as_deref().unwrap_or("?");
            out.push_str(&format!(
                "  {}:{} (in {}): {}\n",
                u.path, u.line, encl, u.text
            ));
        }
        out.push_str(&footer(
            p.total_usages,
            p.offset,
            p.usages.len(),
            p.has_more,
        ));
    }

    if p.definitions.is_empty() && p.total_usages == 0 {
        out.push_str("\n[no references found]\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn dummy_payload() -> RefsOutput {
        RefsOutput {
            name: "foo".into(),
            definitions: vec![RefDefinition {
                path: "src/a.rs".into(),
                line: 10,
                end_line: 20,
                kind: "function".into(),
                weight: 5,
            }],
            usages: vec![RefUsage {
                path: "src/b.rs".into(),
                line: 30,
                text: "    foo();".into(),
                enclosing: Some("bar".into()),
            }],
            total_usages: 1,
            offset: 0,
            has_more: false,
        }
    }

    #[test]
    fn json_shape_includes_required_fields() {
        let p = dummy_payload();
        let v: Value = serde_json::to_value(&p).unwrap();
        for k in [
            "name",
            "definitions",
            "usages",
            "total_usages",
            "offset",
            "has_more",
        ] {
            assert!(v.get(k).is_some(), "missing field {k}");
        }
        assert_eq!(v["definitions"][0]["kind"], "function");
        assert_eq!(v["usages"][0]["enclosing"], "bar");
    }

    #[test]
    fn text_render_groups_defs_then_usages() {
        let s = render_text(&dummy_payload());
        let defs_idx = s.find("Definitions").unwrap();
        let usages_idx = s.find("Usages").unwrap();
        assert!(defs_idx < usages_idx);
        assert!(s.contains("src/a.rs:10"));
        assert!(s.contains("src/b.rs:30 (in bar)"));
    }

    #[test]
    fn text_render_zero_refs_emits_none_marker() {
        let p = RefsOutput {
            name: "missing".into(),
            definitions: Vec::new(),
            usages: Vec::new(),
            total_usages: 0,
            offset: 0,
            has_more: false,
        };
        let s = render_text(&p);
        assert!(s.contains("[no references found]"));
    }

    #[test]
    fn json_omits_enclosing_when_none() {
        let p = RefsOutput {
            name: "x".into(),
            definitions: Vec::new(),
            usages: vec![RefUsage {
                path: "p".into(),
                line: 1,
                text: "x".into(),
                enclosing: None,
            }],
            total_usages: 1,
            offset: 0,
            has_more: false,
        };
        let v: Value = serde_json::to_value(&p).unwrap();
        assert!(v["usages"][0].get("enclosing").is_none());
    }
}
