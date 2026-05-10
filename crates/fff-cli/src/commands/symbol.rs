use std::path::Path;
use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use serde::Serialize;

use fff_engine::Engine;
use fff_symbol::symbol_index::SymbolLocation;

use crate::cli::OutputFormat;
use crate::commands::did_you_mean::{suggest, Suggestion};
use crate::commands::expand::{expand_hit, per_hit_budget_bytes, Expanded};
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

    /// Inline each definition's body and a `── calls ──` block listing direct
    /// callees. Token-budget capped via `--budget`.
    #[arg(long, default_value_t = false)]
    pub expand: bool,

    /// Disable did-you-mean suggestions when no exact match is found.
    /// Suggestions are on by default and only emit when there are zero hits.
    #[arg(long = "no-did-you-mean", default_value_t = false)]
    pub no_did_you_mean: bool,

    /// Token budget for `--expand`. Split across visible hits with a per-hit
    /// floor of 256 bytes. Ignored when `--expand` is not set.
    #[arg(long, default_value_t = 10_000)]
    pub budget: u64,
}

#[derive(Debug, Serialize)]
struct SymbolOutput {
    query: String,
    hits: Vec<SymbolHit>,
    total: usize,
    offset: usize,
    has_more: bool,
    facets: Facets,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    did_you_mean: Vec<Suggestion>,
}

#[derive(Debug, Serialize)]
struct SymbolHit {
    name: String,
    path: String,
    line: u32,
    end_line: u32,
    kind: String,
    weight: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    expanded: Option<Expanded>,
}

fn loc_to_hit(name: &str, loc: SymbolLocation) -> SymbolHit {
    SymbolHit {
        name: name.to_string(),
        path: loc.path.to_string_lossy().to_string(),
        line: loc.line,
        end_line: loc.end_line,
        kind: loc.kind,
        weight: loc.weight,
        expanded: None,
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

    let did_you_mean = if all_hits.is_empty() && !args.no_did_you_mean {
        suggest(&engine, &args.name)
    } else {
        Vec::new()
    };

    let facets = Facets::from_kinds(all_hits.iter().map(|h| h.kind.as_str()));
    let mut page = Page::paginate(all_hits, args.offset, args.limit);

    if args.expand && !page.items.is_empty() {
        let per_hit = per_hit_budget_bytes(args.budget, page.items.len());
        for hit in page.items.iter_mut() {
            let path = PathBuf::from(&hit.path);
            hit.expanded = expand_hit(&engine, &hit.name, &path, hit.line, hit.end_line, per_hit);
        }
    }

    let payload = SymbolOutput {
        query: args.name,
        total: page.total,
        offset: page.offset,
        has_more: page.has_more,
        hits: page.items,
        facets,
        did_you_mean,
    };
    super::emit(format, &payload, |p| {
        let mut out = String::new();
        for h in &p.hits {
            out.push_str(&format!(
                "{} @ {}:{} ({})\n",
                h.name, h.path, h.line, h.kind
            ));
            if let Some(exp) = &h.expanded {
                for line in exp.body.lines() {
                    out.push_str("  ");
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
        if p.total == 0 {
            out.push_str("[no symbols found]\n");
            if !p.did_you_mean.is_empty() {
                out.push_str("did you mean:\n");
                for s in &p.did_you_mean {
                    out.push_str(&format!(
                        "  {} ({}) — {} hit{}\n",
                        s.name,
                        s.source,
                        s.hits.len(),
                        if s.hits.len() == 1 { "" } else { "s" }
                    ));
                }
            }
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
