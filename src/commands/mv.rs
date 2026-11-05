use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Parser;
use serde::{Deserialize, Serialize};

use crate::job::{BatchContext, BatchItem, BatchReport, ExecutionContext, ExecutionStrategy, Job};
use crate::registry::command_pattern;
use crate::shell::ShellCommand;

use super::fs::{checked_regular, destination_metadata, same_file};

#[derive(Clone, Debug, Parser, Serialize, Deserialize)]
#[command(disable_help_flag = true, disable_version_flag = true)]
pub struct Mv {
    pub src: PathBuf,
    pub dst: PathBuf,
}

impl Job for Mv {
    const TYPE: &'static str = "mv";

    fn patterns() -> Vec<String> {
        vec![command_pattern("mv")]
    }

    fn parse(shell: &ShellCommand) -> Result<Self> {
        Ok(Self::try_parse_from(
            std::iter::once(shell.program.as_str()).chain(shell.args.iter().map(String::as_str)),
        )?)
    }

    fn inputs(&self) -> impl Iterator<Item = &Path> {
        std::iter::once(self.src.as_path())
    }

    fn outputs(&self) -> impl Iterator<Item = &Path> {
        [self.src.as_path(), self.dst.as_path()].into_iter()
    }

    fn strategy(&self) -> ExecutionStrategy {
        ExecutionStrategy::Internal
    }

    fn unknown_effects(&self) -> bool {
        true
    }

    fn execute(&self, ctx: &ExecutionContext<'_>) -> Result<()> {
        move_file(&ctx.absolute(&self.src), &ctx.absolute(&self.dst))
    }

    fn execute_batch(items: &[BatchItem<'_, Self>], ctx: &BatchContext<'_>) -> BatchReport {
        ctx.run_each(items, |job, execution| job.execute(execution))
    }
}

fn move_file(src: &Path, dst: &Path) -> Result<()> {
    let source = checked_regular(src)?;
    if let Some(destination) = destination_metadata(dst)? {
        if same_file(&source, &destination) {
            bail!("mv source and destination are the same file");
        }
    }
    // Cross-device moves require metadata and cleanup semantics beyond this
    // narrow subset. rename reports EXDEV before mutating either location.
    fs::rename(src, dst).with_context(|| {
        format!(
            "mv {} to {} (cross-device moves unsupported)",
            src.display(),
            dst.display()
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_move_preserves_payload() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a");
        let dst = dir.path().join("b");
        fs::write(&src, "payload").unwrap();
        move_file(&src, &dst).unwrap();
        assert!(!src.exists());
        assert_eq!(fs::read(dst).unwrap(), b"payload");
    }

    #[cfg(unix)]
    #[test]
    fn move_rejects_alias_without_removing_source() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a");
        let dst = dir.path().join("b");
        fs::write(&src, "payload").unwrap();
        fs::hard_link(&src, &dst).unwrap();
        assert!(move_file(&src, &dst).is_err());
        assert_eq!(fs::read(src).unwrap(), b"payload");
        assert_eq!(fs::read(dst).unwrap(), b"payload");
    }
}
