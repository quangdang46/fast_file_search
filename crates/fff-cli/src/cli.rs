//! CLI definition for the `scry` binary.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};

use crate::commands;

/// Unified code search and read tool.
#[derive(Debug, Parser)]
#[command(
    name = "scry",
    version,
    about = "Unified code search/read tool. Replaces grep/glob/find/cat with token-budget aware output.",
    long_about = None
)]
pub struct Cli {
    /// Root directory to search/index. Defaults to current working dir.
    #[arg(long, global = true)]
    pub root: Option<PathBuf>,

    /// Output format.
    #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Text)]
    pub format: OutputFormat,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Find files by name (replaces `find` and `fd`).
    Find(commands::find::Args),

    /// Match files by glob pattern (replaces `glob` and shell `**`).
    Glob(commands::glob::Args),

    /// Search file contents (replaces `grep` and `rg`).
    Grep(commands::grep::Args),

    /// Read a file with token-budget aware truncation (replaces `cat`).
    Read(commands::read::Args),

    /// Render a file's structural outline (functions, classes, …).
    Outline(commands::outline::Args),

    /// Look up symbol definitions (NEW; tree-sitter AST powered).
    Symbol(commands::symbol::Args),

    /// List callers of a symbol (NEW).
    Callers(commands::callers::Args),

    /// List callees referenced inside a symbol body (NEW).
    Callees(commands::callees::Args),

    /// List sibling symbols (peers in the same parent scope).
    Siblings(commands::siblings::Args),

    /// Show a file's imports + the workspace files that depend on it.
    Deps(commands::deps::Args),

    /// Auto-classify a free-form query and route it to the right backend.
    Dispatch(commands::dispatch::Args),

    /// Build / refresh the on-disk indexes (Bigram, Bloom, Symbol, Outline).
    Index(commands::index::Args),

    /// High-signal summary of the workspace (languages, top symbols, …).
    Overview(commands::overview::Args),

    /// Run as MCP server over stdio (replaces agent built-ins Grep/Glob/Read).
    Mcp(commands::mcp::Args),

    /// Print the embedded agent guide.
    Guide(commands::guide::Args),
}

impl Cli {
    pub fn run(self) -> Result<()> {
        let root = self
            .root
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        match self.command {
            Command::Find(a) => commands::find::run(a, &root, self.format),
            Command::Glob(a) => commands::glob::run(a, &root, self.format),
            Command::Grep(a) => commands::grep::run(a, &root, self.format),
            Command::Read(a) => commands::read::run(a, &root, self.format),
            Command::Outline(a) => commands::outline::run(a, &root, self.format),
            Command::Symbol(a) => commands::symbol::run(a, &root, self.format),
            Command::Callers(a) => commands::callers::run(a, &root, self.format),
            Command::Callees(a) => commands::callees::run(a, &root, self.format),
            Command::Siblings(a) => commands::siblings::run(a, &root, self.format),
            Command::Deps(a) => commands::deps::run(a, &root, self.format),
            Command::Dispatch(a) => commands::dispatch::run(a, &root, self.format),
            Command::Index(a) => commands::index::run(a, &root, self.format),
            Command::Overview(a) => commands::overview::run(a, &root, self.format),
            Command::Mcp(a) => commands::mcp::run(a, &root),
            Command::Guide(a) => commands::guide::run(a, &root, self.format),
        }
    }
}
