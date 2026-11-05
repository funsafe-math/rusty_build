use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};

use crate::job::{
    BatchContext, BatchItem, BatchReport, ExecutionContext, ExecutionStrategy, Job, JobRecord,
};
use crate::registry::command_pattern;
use crate::shell::ShellCommand;

use super::fs::reject_symlink_components;

#[derive(Clone, Debug, Parser, Serialize, Deserialize)]
#[command(disable_help_flag = true, disable_version_flag = true)]
pub struct Mkdir {
    #[arg(short = 'p', long)]
    pub parents: bool,
    #[arg(required = true, num_args = 1..)]
    pub paths: Vec<PathBuf>,
}

impl Job for Mkdir {
    const TYPE: &'static str = "mkdir";

    fn patterns() -> Vec<String> {
        vec![command_pattern("mkdir")]
    }

    fn parse(shell: &ShellCommand) -> Result<Self> {
        Ok(Self::try_parse_from(
            std::iter::once(shell.program.as_str()).chain(shell.args.iter().map(String::as_str)),
        )?)
    }

    fn inputs(&self) -> impl Iterator<Item = &Path> {
        std::iter::empty()
    }

    fn outputs(&self) -> impl Iterator<Item = &Path> {
        self.paths.iter().map(PathBuf::as_path)
    }

    fn strategy(&self) -> ExecutionStrategy {
        ExecutionStrategy::Internal
    }

    fn cacheable(&self) -> bool {
        true
    }

    fn creates_parents(&self) -> bool {
        self.parents
    }

    fn validate(&self, _record: &JobRecord, cwd: &Path) -> Result<()> {
        reject_symlink_components(self.outputs(), cwd)
    }

    fn execute(&self, ctx: &ExecutionContext<'_>) -> Result<()> {
        for path in &self.paths {
            let path = ctx.absolute(path);
            if self.parents {
                fs::create_dir_all(&path)
                    .with_context(|| format!("mkdir -p {}", path.display()))?;
            } else {
                fs::create_dir(&path).with_context(|| format!("mkdir {}", path.display()))?;
            }
        }
        Ok(())
    }

    fn execute_batch(items: &[BatchItem<'_, Self>], ctx: &BatchContext<'_>) -> BatchReport {
        ctx.run_each(items, |job, execution| job.execute(execution))
    }
}
