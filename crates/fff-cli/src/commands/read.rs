use std::path::Path;

use anyhow::Result;
use clap::Parser;
use serde::Serialize;

use fff_budget::FilterLevel;
use fff_engine::{Engine, EngineConfig};

use crate::cli::OutputFormat;

#[derive(Debug, Parser)]
pub struct Args {
    /// File path to read; may be of the form `path:line` to highlight a span.
    pub target: String,

    /// Token budget for the output (default 25000).
    /// Effective byte cap = tokens × ~85% body × 4 bytes/token. The remaining
    /// ~15% is reserved for the response envelope and the truncation footer.
    #[arg(long)]
    pub budget: Option<u64>,

    /// Filter intensity: `none` keeps the file as-is, `minimal` (default)
    /// strips full-line comments, `aggressive` collapses impl/class bodies.
    #[arg(long, default_value = "minimal")]
    pub filter: String,
}

#[derive(Debug, Serialize)]
struct ReadOutput {
    path: String,
    body: String,
    kept_bytes: usize,
    footer_bytes: usize,
}

pub fn run(args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    let level = match args.filter.as_str() {
        "none" => FilterLevel::None,
        "aggressive" => FilterLevel::Aggressive,
        _ => FilterLevel::Minimal,
    };

    let cfg = EngineConfig {
        filter_level: level,
        total_token_budget: args.budget.unwrap_or(25_000),
        ..EngineConfig::default()
    };
    let engine = Engine::new(cfg);

    let path_part = args
        .target
        .rsplit_once(':')
        .map_or(args.target.as_str(), |(p, _)| p);
    let p = if Path::new(path_part).is_absolute() {
        std::path::PathBuf::from(path_part)
    } else {
        root.join(path_part)
    };
    let res = engine.read(&p);

    let payload = ReadOutput {
        path: res.path.to_string_lossy().to_string(),
        body: res.body.clone(),
        kept_bytes: res.outcome.kept_bytes,
        footer_bytes: res.outcome.footer_bytes,
    };
    super::emit(format, &payload, |p| p.body.clone())
}
