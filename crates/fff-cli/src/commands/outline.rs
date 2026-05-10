use std::path::Path;

use anyhow::{anyhow, Result};
use clap::{Parser, ValueEnum};
use serde::Serialize;

use fff_symbol::lang::detect_file_type;
use fff_symbol::outline::get_outline_entries;
use fff_symbol::types::{estimate_tokens, FileType, OutlineEntry, OutlineKind};

use crate::cli::OutputFormat;
use crate::commands::outline_format::{self, AgentHeader};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Style {
    /// Agent-friendly default: header line, `[A-B]` left column, bundled
    /// imports, indented signatures, and a `> Next:` drill hint.
    Agent,
    Markdown,
    Structured,
    Tabular,
}

#[derive(Debug, Parser)]
pub struct Args {
    /// File to outline.
    pub path: String,

    /// Text rendering style. Ignored when --format=json.
    #[arg(long, value_enum, default_value_t = Style::Agent)]
    pub style: Style,
}

#[derive(Debug, Serialize)]
struct OutlineOutput {
    path: String,
    lang: String,
    entries: Vec<OutlineEntryDto>,
}

#[derive(Debug, Serialize)]
struct OutlineEntryDto {
    kind: String,
    name: String,
    start_line: u32,
    end_line: u32,
    signature: Option<String>,
    children: Vec<OutlineEntryDto>,
}

impl OutlineEntryDto {
    fn from_entry(e: &OutlineEntry) -> Self {
        Self {
            kind: kind_str(e.kind).to_string(),
            name: e.name.clone(),
            start_line: e.start_line,
            end_line: e.end_line,
            signature: e.signature.clone(),
            children: e.children.iter().map(Self::from_entry).collect(),
        }
    }
}

fn kind_str(k: OutlineKind) -> &'static str {
    match k {
        OutlineKind::Import => "import",
        OutlineKind::Function => "function",
        OutlineKind::Class => "class",
        OutlineKind::Struct => "struct",
        OutlineKind::Interface => "interface",
        OutlineKind::TypeAlias => "type_alias",
        OutlineKind::Enum => "enum",
        OutlineKind::Constant => "constant",
        OutlineKind::Variable => "variable",
        OutlineKind::ImmutableVariable => "immutable_variable",
        OutlineKind::Export => "export",
        OutlineKind::Property => "property",
        OutlineKind::Module => "module",
        OutlineKind::TestSuite => "test_suite",
        OutlineKind::TestCase => "test_case",
    }
}

pub fn run(args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    let p = if Path::new(&args.path).is_absolute() {
        std::path::PathBuf::from(&args.path)
    } else {
        root.join(&args.path)
    };

    let ft = detect_file_type(&p);
    let lang = match ft {
        FileType::Code(l) => l,
        _ => return Err(anyhow!("not a code file: {}", p.display())),
    };

    let content =
        std::fs::read_to_string(&p).map_err(|e| anyhow!("failed to read {}: {e}", p.display()))?;
    let entries = get_outline_entries(&content, lang);

    let payload = OutlineOutput {
        path: p.to_string_lossy().to_string(),
        lang: format!("{lang:?}"),
        entries: entries.iter().map(OutlineEntryDto::from_entry).collect(),
    };

    let display_path = args.path.clone();
    let total_lines = content.lines().count() as u64;
    let total_tokens = estimate_tokens(content.len() as u64);

    super::emit(format, &payload, |_| match args.style {
        Style::Agent => outline_format::agent(
            &entries,
            AgentHeader {
                path: &display_path,
                lines: total_lines,
                tokens: total_tokens,
            },
        ),
        Style::Markdown => outline_format::markdown(&entries),
        Style::Structured => outline_format::structured(&entries),
        Style::Tabular => outline_format::tabular(&entries),
    })
}

/// Library-style helper: render the agent outline for a path. Used by
/// `scry read` when no flags steer it elsewhere.
pub fn render_agent(path: &Path, display_path: &str) -> Result<String> {
    let ft = detect_file_type(path);
    let lang = match ft {
        FileType::Code(l) => l,
        _ => return Err(anyhow!("not a code file: {}", path.display())),
    };
    let content =
        std::fs::read_to_string(path).map_err(|e| anyhow!("failed to read {}: {e}", path.display()))?;
    let entries = get_outline_entries(&content, lang);
    let total_lines = content.lines().count() as u64;
    let total_tokens = estimate_tokens(content.len() as u64);
    Ok(outline_format::agent(
        &entries,
        AgentHeader {
            path: display_path,
            lines: total_lines,
            tokens: total_tokens,
        },
    ))
}
