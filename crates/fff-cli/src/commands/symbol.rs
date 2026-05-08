use std::path::Path;

use anyhow::Result;
use clap::Parser;
use serde::Serialize;

use fff_engine::Engine;
use fff_symbol::symbol_index::SymbolLocation;

use crate::cli::OutputFormat;

#[derive(Debug, Parser)]
pub struct Args {
    /// Symbol name to look up. Glob patterns ending with `*` are treated as prefixes.
    pub name: String,
}

#[derive(Debug, Serialize)]
struct SymbolOutput {
    query: String,
    hits: Vec<SymbolHit>,
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

    let hits: Vec<SymbolHit> = if let Some(prefix) = args.name.strip_suffix('*') {
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

    let payload = SymbolOutput {
        query: args.name,
        hits,
    };
    super::emit(format, &payload, |p| {
        let mut out = String::new();
        for h in &p.hits {
            out.push_str(&format!(
                "{} @ {}:{} ({})\n",
                h.name, h.path, h.line, h.kind
            ));
        }
        if p.hits.is_empty() {
            out.push_str("[no symbols found]\n");
        }
        out
    })
}
