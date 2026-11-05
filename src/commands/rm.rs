use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};

use crate::job::{BatchContext, BatchItem, BatchReport, ExecutionContext, ExecutionStrategy, Job};
use crate::registry::command_pattern;
use crate::shell::ShellCommand;

#[derive(Clone, Debug, Parser, Serialize, Deserialize)]
#[command(disable_help_flag = true, disable_version_flag = true)]
pub struct Rm {
    #[arg(short = 'f', long)]
    pub force: bool,
    #[arg(required = true, num_args = 1..)]
    pub paths: Vec<PathBuf>,
}

impl Job for Rm {
    const TYPE: &'static str = "rm";

    fn patterns() -> Vec<String> {
        vec![command_pattern("rm")]
    }

    fn parse(shell: &ShellCommand) -> Result<Self> {
        Ok(Self::try_parse_from(
            std::iter::once(shell.program.as_str()).chain(shell.args.iter().map(String::as_str)),
        )?)
    }

    fn inputs(&self) -> impl Iterator<Item = &Path> {
        self.paths.iter().map(PathBuf::as_path)
    }

    fn outputs(&self) -> impl Iterator<Item = &Path> {
        self.paths.iter().map(PathBuf::as_path)
    }

    fn strategy(&self) -> ExecutionStrategy {
        ExecutionStrategy::Internal
    }

    // Removal invalidates filesystem state beyond reusable content outputs.
    fn unknown_effects(&self) -> bool {
        true
    }

    fn execute(&self, ctx: &ExecutionContext<'_>) -> Result<()> {
        for path in &self.paths {
            remove_file(&ctx.absolute(path), self.force)?;
        }
        Ok(())
    }

    fn execute_batch(items: &[BatchItem<'_, Self>], ctx: &BatchContext<'_>) -> BatchReport {
        ctx.run_each(items, |job, execution| job.execute(execution))
    }
}

fn remove_file(path: &Path, force: bool) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if force && error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("rm {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_force_remove() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("b");
        fs::write(&path, "payload").unwrap();
        remove_file(&path, false).unwrap();
        assert!(!path.exists());
        remove_file(&path, true).unwrap();
        assert!(remove_file(&path, false).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn remove_unlinks_symlink_without_removing_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        let link = dir.path().join("link");
        fs::write(&target, "payload").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        remove_file(&link, false).unwrap();
        assert_eq!(fs::read(target).unwrap(), b"payload");
        assert!(fs::symlink_metadata(link).is_err());
    }
}
