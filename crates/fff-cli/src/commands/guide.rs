use std::path::Path;

use anyhow::Result;
use clap::Parser;
use serde::Serialize;

use crate::cli::OutputFormat;

const GUIDE_TEXT: &str = include_str!("../../assets/AGENT_GUIDE.md");

#[derive(Debug, Parser)]
pub struct Args {}

#[derive(Debug, Serialize)]
struct GuideOutput {
    body: String,
}

pub fn run(_args: Args, _root: &Path, format: OutputFormat) -> Result<()> {
    let payload = GuideOutput {
        body: GUIDE_TEXT.to_string(),
    };
    super::emit(format, &payload, |p| p.body.clone())
}
