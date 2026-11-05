use std::fs::{self, File, FileTimes, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Parser;
use serde::{Deserialize, Serialize};

use crate::job::{
    BatchContext, BatchItem, BatchReport, ExecutionContext, ExecutionStrategy, Job, JobRecord,
};
use crate::registry::command_pattern;
use crate::shell::ShellCommand;

use super::fs::{checked_regular, reject_symlink_components, singleton};

#[derive(Clone, Debug, Parser, Serialize, Deserialize)]
#[command(disable_help_flag = true, disable_version_flag = true)]
pub struct Touch {
    #[arg(required = true, num_args = 1..)]
    pub paths: Vec<PathBuf>,
}

impl Job for Touch {
    const TYPE: &'static str = "touch";

    fn patterns() -> Vec<String> {
        vec![command_pattern("touch")]
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

    // Timestamp updates are observable even when file contents do not change.
    fn cacheable(&self) -> bool {
        false
    }

    fn validate(&self, _record: &JobRecord, cwd: &Path) -> Result<()> {
        reject_symlink_components(self.outputs(), cwd)
    }

    fn execute(&self, ctx: &ExecutionContext<'_>) -> Result<()> {
        for path in &self.paths {
            touch(&ctx.absolute(path))?;
        }
        Ok(())
    }

    fn execute_batch(items: &[BatchItem<'_, Self>], ctx: &BatchContext<'_>) -> BatchReport {
        ctx.run_each(items, |job, execution| job.execute(execution))
    }
}

fn touch(path: &Path) -> Result<()> {
    let file = match fs::symlink_metadata(path) {
        Ok(_) => {
            if !singleton(&checked_regular(path)?) {
                bail!(
                    "native touch does not support multiply-linked files: {}",
                    path.display()
                );
            }
            File::open(path).with_context(|| format!("opening {} for touch", path.display()))?
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            OpenOptions::new().write(true).create_new(true).open(path)?
        }
        Err(error) => return Err(error.into()),
    };
    let now = std::time::SystemTime::now();
    file.set_times(FileTimes::new().set_accessed(now).set_modified(now))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_touch_creates_file_and_preserves_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a");
        touch(&path).unwrap();
        assert!(path.is_file());
        fs::write(&path, "payload").unwrap();
        touch(&path).unwrap();
        assert_eq!(fs::read(path).unwrap(), b"payload");
        assert!(!Touch { paths: vec![] }.cacheable());
    }

    #[cfg(unix)]
    #[test]
    fn touch_rejects_hard_links() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a");
        fs::write(&path, "payload").unwrap();
        fs::hard_link(&path, dir.path().join("b")).unwrap();
        assert!(touch(&path).is_err());
        assert_eq!(fs::read(path).unwrap(), b"payload");
    }
}
