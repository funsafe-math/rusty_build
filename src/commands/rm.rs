use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Parser;
use serde::{Deserialize, Serialize};

use crate::job::{
    BatchContext, BatchItem, BatchReport, ExecutionContext, ExecutionStrategy, Job, JobRecord,
    JobSpec,
};
use crate::registry::command_pattern;
use crate::shell::ShellCommand;

use super::fs::{absolute, reject_symlink_components};

#[derive(Clone, Debug, Parser, Serialize, Deserialize)]
#[command(disable_help_flag = true, disable_version_flag = true)]
pub struct Rm {
    #[arg(short = 'f', long)]
    pub force: bool,
    #[arg(short = 'r', visible_short_alias = 'R', long)]
    #[serde(default)]
    pub recursive: bool,
    #[arg(required_unless_present = "force", num_args = 1..)]
    pub paths: Vec<PathBuf>,
}

impl Job for Rm {
    const TYPE: &'static str = "rm";

    fn patterns() -> Vec<String> {
        vec![command_pattern("rm")]
    }

    fn parse(shell: &ShellCommand) -> Result<Self> {
        let command = Self::try_parse_from(
            std::iter::once(shell.program.as_str()).chain(shell.args.iter().map(String::as_str)),
        )?;
        for path in &command.paths {
            reject_unsafe_operand(path)?;
        }
        Ok(command)
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

    fn refine_effects(&self, _spec: &mut JobSpec, cwd: &Path) -> Result<()> {
        self.validate_paths(cwd)
    }

    fn validate(&self, _record: &JobRecord, cwd: &Path) -> Result<()> {
        self.validate_paths(cwd)
    }

    fn execute(&self, ctx: &ExecutionContext<'_>) -> Result<()> {
        // Check every operand before removing anything, including restored jobs.
        self.validate_paths(ctx.cwd)?;
        for path in &self.paths {
            let path = checked_path(path, ctx.cwd)?;
            remove_path(&path, self.force, self.recursive)?;
        }
        Ok(())
    }

    fn execute_batch(items: &[BatchItem<'_, Self>], ctx: &BatchContext<'_>) -> BatchReport {
        ctx.run_each(items, |job, execution| job.execute(execution))
    }
}

impl Rm {
    fn validate_paths(&self, cwd: &Path) -> Result<()> {
        for path in &self.paths {
            checked_path(path, cwd)?;
        }
        Ok(())
    }
}

fn reject_unsafe_operand(path: &Path) -> Result<()> {
    // Path::components normalizes trailing "/.", so inspect the original final
    // component before stripping separators for symlink-safe removal.
    let final_component = path
        .as_os_str()
        .as_encoded_bytes()
        .rsplit(|byte| *byte == b'/' || (cfg!(windows) && *byte == b'\\'))
        .find(|component| !component.is_empty());
    if matches!(final_component, None | Some(b".") | Some(b".."))
        || !path.components().any(|component| {
            matches!(
                component,
                std::path::Component::Normal(_) | std::path::Component::ParentDir
            )
        })
    {
        bail!(
            "rm: refusing to remove root, '.' or '..': {}",
            path.display()
        );
    }
    Ok(())
}

fn checked_path(path: &Path, cwd: &Path) -> Result<PathBuf> {
    reject_unsafe_operand(path)?;
    let requires_directory = path
        .as_os_str()
        .as_encoded_bytes()
        .last()
        .is_some_and(|byte| *byte == b'/' || (cfg!(windows) && *byte == b'\\'));
    // A trailing separator must not cause a symlink operand to be followed.
    // Keep ParentDir components intact: resolving them lexically could change
    // which object an operand names.
    let path: PathBuf = absolute(cwd, path).components().collect();
    if let Some(parent) = path.parent() {
        reject_symlink_components(std::iter::once(parent), cwd)?;
    }
    if requires_directory
        && fs::symlink_metadata(&path)
            .is_ok_and(|metadata| !metadata.is_dir() && !metadata.file_type().is_symlink())
    {
        bail!(
            "rm: trailing separator requires a directory: {}",
            path.display()
        );
    }
    Ok(path)
}

fn remove_path(path: &Path, force: bool, recursive: bool) -> Result<()> {
    let result = (|| -> io::Result<()> {
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() {
            // Windows directory symlinks are unlinked with RemoveDirectory,
            // unlike file symlinks. Never traverse either kind.
            #[cfg(windows)]
            {
                use std::os::windows::fs::FileTypeExt;
                if metadata.file_type().is_symlink_dir() {
                    return fs::remove_dir(path);
                }
            }
            fs::remove_file(path)
        } else if recursive && metadata.is_dir() {
            // std's recursive removal does not follow symlinks inside the tree.
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        }
    })();
    match result {
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
        remove_path(&path, false, false).unwrap();
        assert!(!path.exists());
        remove_path(&path, true, false).unwrap();
        assert!(remove_path(&path, false, false).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn remove_unlinks_symlink_without_removing_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        let link = dir.path().join("link");
        fs::write(&target, "payload").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        remove_path(&link, false, false).unwrap();
        assert_eq!(fs::read(target).unwrap(), b"payload");
        assert!(fs::symlink_metadata(link).is_err());
    }
}
