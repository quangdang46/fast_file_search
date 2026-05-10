use std::path::Path;

use anyhow::Result;
use clap::Parser;
use serde::Serialize;

use fff_engine::Engine;
use fff_symbol::symbol_index::SymbolLocation;

use crate::cli::OutputFormat;
use crate::commands::facets::Facets;
use crate::commands::pagination::{footer, Page};

#[derive(Debug, Parser)]
pub struct Args {
    /// Symbol name to look up. Glob patterns ending with `*` are treated as prefixes.
    pub name: String,

    /// Maximum hits returned in this page.
    #[arg(long, default_value_t = 100)]
    pub limit: usize,

    /// Skip this many hits before starting the page.
    #[arg(long, default_value_t = 0)]
    pub offset: usize,
}

#[derive(Debug, Serialize)]
struct SymbolOutput {
    query: String,
    hits: Vec<SymbolHit>,
    total: usize,
    offset: usize,
    has_more: bool,
    facets: Facets,
}

#[derive(Debug, Serialize)]
struct SymbolHit {
    name: String,
    path: String,
    line: u32,
    end_line: u32,
    kind: String,
    weight: u16,
}

fn loc_to_hit(name: &str, loc: SymbolLocation) -> SymbolHit {
    SymbolHit {
        name: name.to_string(),
        path: loc.path.to_string_lossy().to_string(),
        line: loc.line,
        end_line: loc.end_line,
        kind: loc.kind,
        weight: loc.weight,
    }
}

pub fn run(args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    let engine = Engine::default();
    engine.index(root);

    let all_hits: Vec<SymbolHit> = if let Some(prefix) = args.name.strip_suffix('*') {
        engine
            .handles
            .symbols
            .lookup_prefix(prefix)
            .into_iter()
            .map(|(n, l)| loc_to_hit(&n, l))
            .collect()
    } else {
        engine
            .handles
            .symbols
            .lookup_exact(&args.name)
            .into_iter()
            .map(|l| loc_to_hit(&args.name, l))
            .collect()
    };

    let facets = Facets::from_kinds(all_hits.iter().map(|h| h.kind.as_str()));
    let page = Page::paginate(all_hits, args.offset, args.limit);
    let payload = SymbolOutput {
        query: args.name,
        total: page.total,
        offset: page.offset,
        has_more: page.has_more,
        hits: page.items,
        facets,
    };
    super::emit(format, &payload, |p| {
        let mut out = String::new();
        for h in &p.hits {
            out.push_str(&format!(
                "{} @ {}:{} ({})\n",
                h.name, h.path, h.line, h.kind
            ));
        }
        if p.total == 0 {
            out.push_str("[no symbols found]\n");
        } else {
            out.push_str(&footer(p.total, p.offset, p.hits.len(), p.has_more));
            if !p.facets.by_kind.is_empty() {
                let parts: Vec<String> = p
                    .facets
                    .by_kind
                    .iter()
                    .map(|(k, n)| format!("{k}: {n}"))
                    .collect();
                out.push_str(&format!("by kind: {}\n", parts.join(", ")));
            }
        }
        out
    })
}
