use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use clap::Parser;
use same_file::Handle;

use crate::job::{FileAccess, Job, JobData};
use crate::shell::ShellHints;

use super::external::execute_native;
use super::fs::{checked_regular, destination_metadata, reject_symlink_components};

/// The supported mv subset moves one regular file on the same filesystem.
#[derive(Debug, Parser)]
#[command(disable_help_flag = true, disable_version_flag = true)]
pub struct Mv {
    pub src: String,
    pub dst: String,
    #[arg(skip)]
    pub data: JobData,
}

impl Mv {
    pub fn parse(argv: &[String], shell: &ShellHints) -> Result<Self> {
        let mut job = Self::try_parse_from(argv)?;
        job.data = JobData::new(vec![
            FileAccess::read_write(job.src.clone()),
            FileAccess::write(job.dst.clone()),
        ]);
        job.data.shell = shell.clone();
        Ok(job)
    }
}

impl Job for Mv {
    fn data(&self) -> &JobData {
        &self.data
    }

    fn data_mut(&mut self) -> &mut JobData {
        &mut self.data
    }

    fn execute(&self) -> Result<()> {
        execute_native(&self.data.shell, || {
            move_file(Path::new(&self.src), Path::new(&self.dst))
        })
    }
}

fn move_file(src: &Path, dst: &Path) -> Result<()> {
    reject_symlink_components([src, dst])?;
    checked_regular(src)?;
    if destination_metadata(dst)?.is_some() && Handle::from_path(src)? == Handle::from_path(dst)? {
        bail!("mv source and destination are the same file");
    }
    // Cross-device moves require copy-and-cleanup semantics outside this subset.
    // Rename reports that failure before mutating either file.
    fs::rename(src, dst).with_context(|| {
        format!(
            "mv {} to {} (cross-device moves unsupported)",
            src.display(),
            dst.display()
        )
    })?;
    Ok(())
}
