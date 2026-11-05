use std::fs;
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
                mkdir_parents(&path)?;
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

fn mkdir_parents(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    let mut components = path.components().peekable();
    if components.peek().is_none() {
        bail!("mkdir: empty path");
    }
    while let Some(component) = components.next() {
        current.push(component.as_os_str());
        match fs::create_dir(&current) {
            Ok(()) => {
                // GNU mkdir -p gives newly created intermediate directories
                // owner write/search even when umask removes those bits.
                #[cfg(unix)]
                if components.peek().is_some() {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = fs::metadata(&current)?.permissions().mode();
                    fs::set_permissions(&current, fs::Permissions::from_mode(mode | 0o300))?;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && current.is_dir() => {}
            Err(error) => {
                return Err(error).with_context(|| format!("mkdir -p {}", current.display()));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mkdir_parent_subset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a/b");
        mkdir_parents(&path).unwrap();
        mkdir_parents(&path).unwrap();
        assert!(path.is_dir());
    }

    #[test]
    fn parent_effect_marker_matches_option() {
        let mut job = Mkdir {
            paths: vec!["a/b".into()],
            parents: false,
        };
        assert!(!job.creates_parents());
        job.parents = true;
        assert!(job.creates_parents());
    }
}
