use anyhow::Result;
use clap::{CommandFactory, Parser};

mod bigram;
mod cache;
mod cli;
mod commands;

fn main() -> Result<()> {
    let args = cli::Cli::parse();

    if let Some(shell) = args.completions {
        clap_complete::generate(
            shell,
            &mut cli::Cli::command(),
            "ffs",
            &mut std::io::stdout(),
        );
        return Ok(());
    }

    args.run()
}
