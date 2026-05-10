//! `scry callees <symbol>` — given a symbol's body, list which other indexed
//! symbols are referenced inside it. Useful for impact analysis.

use std::path::Path;

use anyhow::Result;
use clap::Parser;
use serde::Serialize;

use fff_engine::Engine;
use fff_symbol::lang::detect_file_type;
use fff_symbol::types::FileType;

use crate::cli::OutputFormat;
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
}

#[derive(Debug, Serialize)]
struct CalleeHit {
    name: String,
    path: String,
    line: u32,
}

#[derive(Debug, Serialize)]
struct CalleesOutput {
    name: String,
    hits: Vec<CalleeHit>,
    total: usize,
    offset: usize,
    has_more: bool,
}

pub fn run(args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    let engine = Engine::default();
    engine.index(root);

    let definitions = engine.handles.symbols.lookup_exact(&args.name);
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
            if ident == args.name {
                continue;
            }
            for loc in engine.handles.symbols.lookup_exact(&ident) {
                hits.push(CalleeHit {
                    name: ident.clone(),
                    path: loc.path.to_string_lossy().to_string(),
                    line: loc.line,
                });
            }
        }
    }

    // Multiple definition sites of the same target symbol can produce the same
    // (name, path, line) triple from different bodies; collapse them so each
    // unique callee is reported once.
    let hits = dedup_by(hits, |h| (h.name.clone(), h.path.clone(), h.line));
    let page = Page::paginate(hits, args.offset, args.limit);
    let payload = CalleesOutput {
        name: args.name,
        total: page.total,
        offset: page.offset,
        has_more: page.has_more,
        hits: page.items,
    };
    super::emit(format, &payload, |p| {
        let mut out = String::new();
        for h in &p.hits {
            out.push_str(&format!("{} @ {}:{}\n", h.name, h.path, h.line));
        }
        if p.total == 0 {
            out.push_str("[no callees found]\n");
        } else {
            out.push_str(&footer(p.total, p.offset, p.hits.len(), p.has_more));
        }
        out
    })
}
