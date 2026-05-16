use anyhow::Result;
use clap::{CommandFactory, Parser};

mod bigram;
mod cache;
mod cli;
mod commands;

// Bug 4: install the default SIGPIPE handler so pipelines like
// `ffs --completions fish | head` exit silently when the consumer closes
// instead of panicking inside `clap_complete::generate`'s writer.
#[cfg(unix)]
fn install_sigpipe_default() {
    // SAFETY: setting a signal disposition before any threads spawn or any
    // I/O happens is sound; we run this as the very first thing in main.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
fn install_sigpipe_default() {}

fn main() -> Result<()> {
    install_sigpipe_default();

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
