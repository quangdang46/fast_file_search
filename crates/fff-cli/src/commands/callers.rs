//! `scry callers <symbol>` — find call sites for `symbol`. With `--hops > 1`
//! the search becomes a BFS over the caller graph.

use std::path::Path;
use std::time::SystemTime;

use anyhow::Result;
use clap::Parser;
use serde::Serialize;

use fff_engine::{Engine, PreFilterStack};
use fff_symbol::lang::detect_file_type;
use fff_symbol::types::FileType;

use crate::cli::OutputFormat;
use crate::commands::callers_bfs::{run_bfs, BfsConfig, CandidateFile};
use crate::commands::pagination::{footer, Page};

#[derive(Debug, Parser)]
pub struct Args {
    /// Symbol name to find call sites for.
    pub name: String,

    /// Maximum hits returned in this page.
    #[arg(long, default_value_t = 100)]
    pub limit: usize,

    /// Skip this many hits before starting the page.
    #[arg(long, default_value_t = 0)]
    pub offset: usize,

    /// How far to walk the caller graph. 1 = direct callers only. Capped at 5.
    #[arg(long, default_value_t = 1)]
    pub hops: u32,

    /// Skip propagation when a single name produces more than this many hits
    /// in one hop. Prevents popular helpers (clone, unwrap, …) from blowing
    /// the BFS up.
    #[arg(long, default_value_t = 50)]
    pub hub_guard: usize,
}

#[derive(Debug, Serialize)]
pub struct CallerHit {
    pub path: String,
    pub line: u32,
    pub text: String,
    pub depth: u32,
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enclosing: Option<String>,
}

#[derive(Debug, Serialize)]
struct CallersOutput {
    name: String,
    hops: u32,
    hub_guard: usize,
    hits: Vec<CallerHit>,
    total: usize,
    offset: usize,
    has_more: bool,
}

pub fn run(args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    let engine = Engine::default();
    engine.index(root);

    let files = super::walk_files(root);
    let candidates: Vec<CandidateFile> = files
        .iter()
        .filter_map(|path| {
            let meta = std::fs::metadata(path).ok()?;
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let content = std::fs::read_to_string(path).ok()?;
            let lang = match detect_file_type(path) {
                FileType::Code(l) => Some(l),
                _ => None,
            };
            Some(CandidateFile {
                path: path.clone(),
                mtime,
                content,
                lang,
            })
        })
        .collect();

    let hits = if args.hops <= 1 {
        single_hop(&engine, &args.name, &candidates)
    } else {
        run_bfs(
            &engine,
            &args.name,
            &candidates,
            BfsConfig {
                max_hops: args.hops,
                hub_guard: args.hub_guard,
            },
        )
    };

    let page = Page::paginate(hits, args.offset, args.limit);
    let payload = CallersOutput {
        name: args.name.clone(),
        hops: args.hops.clamp(1, 5),
        hub_guard: args.hub_guard,
        total: page.total,
        offset: page.offset,
        has_more: page.has_more,
        hits: page.items,
    };
    super::emit(format, &payload, render_text)
}

fn single_hop(engine: &Engine, name: &str, candidates: &[CandidateFile]) -> Vec<CallerHit> {
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

    let mut hits: Vec<CallerHit> = Vec::new();
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

        for (lineno, line) in cf.content.lines().enumerate() {
            let lineno = (lineno + 1) as u32;
            if !line.contains(name) {
                continue;
            }
            if definition_lines.contains(&lineno) {
                continue;
            }
            hits.push(CallerHit {
                path: path_str.clone(),
                line: lineno,
                text: line.to_string(),
                depth: 1,
                target: name.to_string(),
                enclosing: None,
            });
        }
    }
    hits
}

fn render_text(p: &CallersOutput) -> String {
    let mut out = String::new();
    for h in &p.hits {
        if p.hops > 1 {
            let encl = h.enclosing.as_deref().unwrap_or("?");
            out.push_str(&format!(
                "[d{}] {}:{}: {}  (in {}, calls {})\n",
                h.depth, h.path, h.line, h.text, encl, h.target
            ));
        } else {
            out.push_str(&format!("{}:{}: {}\n", h.path, h.line, h.text));
        }
    }
    if p.total == 0 {
        out.push_str("[no callers found]\n");
    } else {
        out.push_str(&footer(p.total, p.offset, p.hits.len(), p.has_more));
    }
    out
}
