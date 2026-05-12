use anyhow::Result;
use clap::{CommandFactory, Parser};

mod cli;
mod commands;

fn main() -> Result<()> {
    let args = cli::Cli::parse();

    if let Some(shell) = args.completions {
        clap_complete::generate(shell, &mut cli::Cli::command(), "scry", &mut std::io::stdout());
        return Ok(());
    }

    args.run()
}
