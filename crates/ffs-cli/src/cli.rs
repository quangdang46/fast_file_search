//! CLI definition for the `ffs` binary.

use std::path::PathBuf;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};

use crate::commands;

/// Unified code search and read tool.
#[derive(Debug, Parser)]
#[command(
    name = "ffs",
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

    /// Generate shell completions for the given shell.
    #[arg(long, global = true, value_enum)]
    pub completions: Option<clap_complete::Shell>,

    #[command(subcommand)]
    pub command: Option<Command>,
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

    /// Multi-pattern OR content search (Aho-Corasick; literal needles).
    #[command(name = "multi-grep", visible_alias = "multigrep")]
    MultiGrep(commands::multi_grep::Args),

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

    /// List definitions and usages of a symbol in one shot (NEW).
    Refs(commands::refs::Args),

    /// Drill-down envelope per definition (def + body + callees + callers).
    Flow(commands::flow::Args),

    /// List sibling symbols (peers in the same parent scope).
    Siblings(commands::siblings::Args),

    /// Show a file's imports + the workspace files that depend on it.
    Deps(commands::deps::Args),

    /// Rank files by how much they'd be affected if a symbol changed (NEW).
    Impact(commands::impact::Args),

    /// Build / refresh the on-disk indexes (Bigram, Bloom, Symbol, Outline).
    Index(commands::index::Args),

    /// Render the workspace as a tree annotated with file count and tokens.
    Map(commands::map::Args),

    /// High-signal summary of the workspace (languages, top symbols, …).
    Overview(commands::overview::Args),

    /// Run as MCP server over stdio (replaces agent built-ins Grep/Glob/Read).
    Mcp(commands::mcp::Args),

    /// Resolve a @-mention payload (Phase C surface — wraps the Phase B
    /// `resolve_mentions` engine API; tokens in the input become substring
    /// candidate queries against the repo).
    Mention(commands::mention::Args),

    /// Print the embedded agent guide.
    Guide(commands::guide::Args),
}

impl Cli {
    pub fn run(self) -> Result<()> {
        let Some(command) = self.command else {
            // No subcommand: print short help.
            Self::command().print_help()?;
            println!();
            return Ok(());
        };

        // MCP gets special root resolution: explicit subcommand PATH, then
        // global --root, then WORKSPACE_FOLDER_PATHS / VSCODE_CWD / cwd.
        // Other commands keep the historical cwd default so scripts that rely
        // on cwd are not surprised by IDE env vars.
        let root = match &command {
            Command::Mcp(a) => {
                if let Some(p) = a.path.clone() {
                    p
                } else if let Some(r) = self.root.clone() {
                    r
                } else {
                    commands::mcp::resolve_default_root()
                }
            }
            _ => self
                .root
                .clone()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))),
        };

        // Bug 14: a non-existent / non-directory root used to silently
        // produce no output and exit 0. Reject it up-front so scripts can
        // distinguish "no matches" from "wrong path".
        let explicit_root =
            self.root.is_some() || matches!(&command, Command::Mcp(a) if a.path.is_some());
        if explicit_root {
            let meta = std::fs::metadata(&root)
                .map_err(|e| anyhow::anyhow!("root {}: {e}", root.display()))?;
            if !meta.is_dir() {
                return Err(anyhow::anyhow!("root {}: not a directory", root.display()));
            }
        }

        match command {
            Command::Find(a) => commands::find::run(a, &root, self.format),
            Command::Glob(a) => commands::glob::run(a, &root, self.format),
            Command::Grep(a) => commands::grep::run(a, &root, self.format),
            Command::MultiGrep(a) => commands::multi_grep::run(a, &root, self.format),
            Command::Read(a) => commands::read::run(a, &root, self.format),
            Command::Outline(a) => commands::outline::run(a, &root, self.format),
            Command::Symbol(a) => commands::symbol::run(a, &root, self.format),
            Command::Callers(a) => commands::callers::run(a, &root, self.format),
            Command::Callees(a) => commands::callees::run(a, &root, self.format),
            Command::Refs(a) => commands::refs::run(a, &root, self.format),
            Command::Flow(a) => commands::flow::run(a, &root, self.format),
            Command::Siblings(a) => commands::siblings::run(a, &root, self.format),
            Command::Deps(a) => commands::deps::run(a, &root, self.format),
            Command::Impact(a) => commands::impact::run(a, &root, self.format),
            Command::Index(a) => commands::index::run(a, &root, self.format),
            Command::Map(a) => commands::map::run(a, &root, self.format),
            Command::Overview(a) => commands::overview::run(a, &root, self.format),
            Command::Mcp(a) => commands::mcp::run(a, &root),
            Command::Mention(a) => commands::mention::run(a, &root, self.format),
            Command::Guide(a) => commands::guide::run(a, &root, self.format),
        }
    }
}
