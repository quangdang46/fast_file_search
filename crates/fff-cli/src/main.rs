use anyhow::Result;
use clap::Parser;

mod cli;
mod commands;

fn main() -> Result<()> {
    let args = cli::Cli::parse();
    args.run()
}
