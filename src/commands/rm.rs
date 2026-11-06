use std::fs;
use std::io;
use std::path::{Component, Path};

use anyhow::{Context, Result, bail};
use clap::Parser;

use crate::job::{FileAccess, Job, JobData};
use crate::shell::ShellHints;

use super::external::execute_native;
use super::fs::reject_symlink_components;

#[derive(Debug, Parser)]
#[command(disable_help_flag = true, disable_version_flag = true)]
pub struct Rm {
    #[arg(short = 'f', long)]
    pub force: bool,
    #[arg(short = 'r', visible_short_alias = 'R', long)]
    pub recursive: bool,
    #[arg(required_unless_present = "force", num_args = 1..)]
    pub paths: Vec<String>,
    #[arg(skip)]
    pub data: JobData,
}

impl Rm {
    pub fn parse(argv: &[String], shell: &ShellHints) -> Result<Self> {
        let mut job = Self::try_parse_from(argv)?;
        for path in &job.paths {
            reject_unsafe_operand(path)?;
        }
        // Removal is a mutation, not a required content read: rm -f missing
        // must reach the native operation without failing an input check.
        job.data = JobData::new(job.paths.iter().map(FileAccess::write).collect());
        job.data.shell = shell.clone();
        Ok(job)
    }
}

impl Job for Rm {
    fn data(&self) -> &JobData {
        &self.data
    }

    fn data_mut(&mut self) -> &mut JobData {
        &mut self.data
    }

    fn execute(&self) -> Result<()> {
        execute_native(&self.data.shell, || {
            // Preflight all operands before removing anything, including jobs
            // constructed directly rather than through the parser.
            for path in &self.paths {
                checked_path(path)?;
            }
            for path in &self.paths {
                remove_path(
                    Path::new(without_trailing_separators(path)),
                    self.force,
                    self.recursive,
                )?;
            }
            Ok(())
        })
    }
}

fn separator(character: char) -> bool {
    character == '/' || (cfg!(windows) && character == '\\')
}

fn without_trailing_separators(path: &str) -> &str {
    path.trim_end_matches(separator)
}

fn reject_unsafe_operand(path: &str) -> Result<()> {
    // Path::components normalizes a final '/.', so inspect the supplied spelling
    // first. Never resolve '..' or canonicalize a destructive operand.
    let final_component = path.rsplit(separator).find(|part| !part.is_empty());
    if matches!(final_component, None | Some(".") | Some(".."))
        || !Path::new(path)
            .components()
            .any(|component| matches!(component, Component::Normal(_) | Component::ParentDir))
    {
        bail!("rm: refusing to remove root, '.' or '..': {path}");
    }
    Ok(())
}

fn checked_path(path: &str) -> Result<()> {
    reject_unsafe_operand(path)?;
    let stripped = without_trailing_separators(path);
    let operand = Path::new(stripped);
    if let Some(parent) = operand.parent() {
        reject_symlink_components(std::iter::once(parent))?;
        // Windows can report a child of a regular file as NotFound. Such an
        // invalid component must not be swallowed by rm -f.
        for ancestor in parent.ancestors() {
            match fs::symlink_metadata(ancestor) {
                Ok(metadata) if !metadata.is_dir() => {
                    bail!("rm: parent is not a directory: {}", ancestor.display());
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("inspecting {}", ancestor.display()));
                }
            }
        }
    }
    // Strip only trailing separators so symlink operands themselves are never
    // followed, even for `rm -r link/`. A regular file still requires no slash.
    if stripped.len() != path.len()
        && fs::symlink_metadata(operand)
            .is_ok_and(|metadata| !metadata.is_dir() && !metadata.file_type().is_symlink())
    {
        bail!("rm: trailing separator requires a directory: {path}");
    }
    Ok(())
}

fn remove_path(path: &Path, force: bool, recursive: bool) -> Result<()> {
    let result = (|| -> io::Result<()> {
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() {
            // Windows directory symlinks require RemoveDirectory. Never walk
            // either file or directory symlink targets.
            #[cfg(windows)]
            {
                use std::os::windows::fs::FileTypeExt;
                if metadata.file_type().is_symlink_dir() {
                    return fs::remove_dir(path);
                }
            }
            fs::remove_file(path)
        } else if recursive && metadata.is_dir() {
            // The standard library also avoids following links inside the tree.
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
