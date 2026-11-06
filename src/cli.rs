use anyhow::{Context, Result};
use clap::Parser;
use std::fs;

#[derive(Debug, Parser)]
#[command(about = "Parse a command log, build its DAG and execute sequential batches")]
pub struct Cli {
    #[arg(long, value_name = "FILE")]
    pub commands: String,
    #[arg(long, short)]
    pub verbose: bool,
}

impl Cli {
    pub fn run(self) -> Result<()> {
        let text = fs::read_to_string(&self.commands)
            .with_context(|| format!("read {}", self.commands))?;
        crate::run(&text, &crate::registry::Registry::default(), self.verbose)
    }
}
