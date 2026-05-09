//! `scry callers <symbol>` — find lines that reference `symbol` outside of its
//! own definition. The implementation is bigram + bloom narrowed, then a final
//! plain-text pass for confirmation.

use std::path::Path;

use anyhow::Result;
use clap::Parser;
use serde::Serialize;

use fff_engine::Engine;

use crate::cli::OutputFormat;

#[derive(Debug, Parser)]
pub struct Args {
    /// Symbol name to find call sites for.
    pub name: String,

    /// Maximum total hits returned.
    #[arg(long, default_value_t = 100)]
    pub limit: usize,
}

#[derive(Debug, Serialize)]
struct CallerHit {
    path: String,
    line: u32,
    text: String,
}

#[derive(Debug, Serialize)]
struct CallersOutput {
    name: String,
    hits: Vec<CallerHit>,
}

pub fn run(args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    let engine = Engine::default();
    engine.index(root);

    let definitions = engine.handles.symbols.lookup_exact(&args.name);
    let definition_paths: Vec<String> = definitions
        .iter()
        .map(|d| d.path.to_string_lossy().to_string())
        .collect();

    let mut hits: Vec<CallerHit> = Vec::new();
    let files = super::walk_files(root);

    for path in &files {
        let path_str = path.to_string_lossy().to_string();
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };

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

        for (lineno, line) in content.lines().enumerate() {
            let lineno = (lineno + 1) as u32;
            if !line.contains(&args.name) {
                continue;
            }
            if definition_lines.contains(&lineno) {
                continue;
            }
            hits.push(CallerHit {
                path: path_str.clone(),
                line: lineno,
                text: line.to_string(),
            });
            if hits.len() >= args.limit {
                break;
            }
        }
        if hits.len() >= args.limit {
            break;
        }
    }

    let payload = CallersOutput {
        name: args.name,
        hits,
    };
    super::emit(format, &payload, |p| {
        let mut out = String::new();
        for h in &p.hits {
            out.push_str(&format!("{}:{}: {}\n", h.path, h.line, h.text));
        }
        if p.hits.is_empty() {
            out.push_str("[no callers found]\n");
        }
        out
    })
}
