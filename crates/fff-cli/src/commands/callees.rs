//! `scry callees <symbol>` — given a symbol's body, list which other indexed
//! symbols are referenced inside it. With `--depth > 1` the search becomes a
//! BFS over the callee graph (mirror of `callers --hops N`).

use std::path::Path;

use anyhow::Result;
use clap::Parser;
use serde::Serialize;

use fff_engine::Engine;
use fff_symbol::lang::detect_file_type;
use fff_symbol::types::FileType;

use crate::cli::OutputFormat;
use crate::commands::callees_bfs::{run_bfs, BfsConfig, CalleeHit as BfsCalleeHit};
use crate::commands::callees_resolve::collect_callees;
use crate::commands::dedup::dedup_by;
use crate::commands::pagination::{footer, Page};

#[derive(Debug, Parser)]
pub struct Args {
    /// Symbol whose body should be inspected.
    pub name: String,

    /// Maximum callees returned in this page.
    #[arg(long, default_value_t = 100)]
    pub limit: usize,

    /// Skip this many callees before starting the page.
    #[arg(long, default_value_t = 0)]
    pub offset: usize,

    /// How far to walk the callee graph. 1 = direct callees only. Capped at 5.
    #[arg(long, default_value_t = 1)]
    pub depth: u32,

    /// Skip propagation when a single name produces more than this many hits
    /// in one hop. Prevents popular helpers from blowing up the BFS.
    #[arg(long, default_value_t = 50)]
    pub hub_guard: usize,
}

#[derive(Debug, Serialize)]
struct CalleeHit {
    name: String,
    path: String,
    line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    depth: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    from: Option<String>,
}

#[derive(Debug, Serialize)]
struct CalleesOutput {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    depth: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hub_guard: Option<usize>,
    hits: Vec<CalleeHit>,
    total: usize,
    offset: usize,
    has_more: bool,
}

pub fn run(args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    let engine = Engine::default();
    engine.index(root);

    let hits = if args.depth <= 1 {
        single_hop(&engine, &args.name)
    } else {
        let bfs_hits = run_bfs(
            &engine,
            &args.name,
            BfsConfig {
                max_hops: args.depth,
                hub_guard: args.hub_guard,
            },
        );
        bfs_hits
            .into_iter()
            .map(|h: BfsCalleeHit| CalleeHit {
                name: h.name,
                path: h.path,
                line: h.line,
                depth: Some(h.depth),
                from: Some(h.from),
            })
            .collect()
    };

    // Multiple paths to the same target collapse into one hit. Depth-aware
    // runs key on `(name, path, line, depth)` so the same target reached via
    // different hops keeps its layered context.
    let hits = dedup_by(hits, |h| (h.name.clone(), h.path.clone(), h.line, h.depth));
    let page = Page::paginate(hits, args.offset, args.limit);
    let multi_hop = args.depth > 1;
    let payload = CalleesOutput {
        name: args.name,
        depth: multi_hop.then(|| args.depth.clamp(1, 5)),
        hub_guard: multi_hop.then_some(args.hub_guard),
        total: page.total,
        offset: page.offset,
        has_more: page.has_more,
        hits: page.items,
    };
    super::emit(format, &payload, render_text)
}

fn single_hop(engine: &Engine, name: &str) -> Vec<CalleeHit> {
    let definitions = engine.handles.symbols.lookup_exact(name);
    let mut hits: Vec<CalleeHit> = Vec::new();
    for def in &definitions {
        let lang = match detect_file_type(&def.path) {
            FileType::Code(l) => l,
            _ => continue,
        };
        let Ok(content) = std::fs::read_to_string(&def.path) else {
            continue;
        };
        let Some(names) = collect_callees(&content, lang, def.line, def.end_line) else {
            continue;
        };
        for ident in names {
            if ident == name {
                continue;
            }
            for loc in engine.handles.symbols.lookup_exact(&ident) {
                hits.push(CalleeHit {
                    name: ident.clone(),
                    path: loc.path.to_string_lossy().to_string(),
                    line: loc.line,
                    depth: None,
                    from: None,
                });
            }
        }
    }
    hits
}

fn render_text(p: &CalleesOutput) -> String {
    let mut out = String::new();
    let multi_hop = p.depth.is_some();
    if multi_hop {
        for h in &p.hits {
            let d = h.depth.unwrap_or(1);
            let from = h.from.as_deref().unwrap_or("?");
            out.push_str(&format!(
                "[d{}] {} @ {}:{}  (called from {})\n",
                d, h.name, h.path, h.line, from
            ));
        }
    } else {
        for h in &p.hits {
            out.push_str(&format!("{} @ {}:{}\n", h.name, h.path, h.line));
        }
    }
    if p.total == 0 {
        out.push_str("[no callees found]\n");
    } else {
        out.push_str(&footer(p.total, p.offset, p.hits.len(), p.has_more));
    }
    out
}
