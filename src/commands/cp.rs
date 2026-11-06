use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

use anyhow::{Context, Result, bail};
use clap::Parser;
use same_file::Handle;

use crate::job::{FileAccess, Job, JobData};
use crate::shell::ShellHints;

use super::external::execute_native;
use super::fs::{checked_regular, destination_metadata, reject_symlink_components};

/// The supported cp subset: exactly two ordinary file operands, no flags.
#[derive(Debug, Parser)]
#[command(disable_help_flag = true, disable_version_flag = true)]
pub struct Cp {
    pub src: String,
    pub dst: String,
    #[arg(skip)]
    pub data: JobData,
}

impl Cp {
    pub fn parse(argv: &[String], shell: &ShellHints) -> Result<Self> {
        let mut job = Self::try_parse_from(argv)?;
        job.data = JobData::new(vec![
            FileAccess::read(job.src.clone()),
            FileAccess::write(job.dst.clone()),
        ]);
        job.data.shell = shell.clone();
        Ok(job)
    }
}

impl Job for Cp {
    fn data(&self) -> &JobData {
        &self.data
    }

    fn data_mut(&mut self) -> &mut JobData {
        &mut self.data
    }

    fn execute(&self) -> Result<()> {
        execute_native(&self.data.shell, || {
            copy_file(Path::new(&self.src), Path::new(&self.dst))
        })
    }
}

/// Stream into the existing destination so that its permissions and hard links
/// survive. A new destination uses source permissions filtered by the umask.
fn copy_file(src: &Path, dst: &Path) -> Result<()> {
    reject_symlink_components([src, dst])?;
    let source_meta = checked_regular(src)?;
    let destination_meta = destination_metadata(dst)?;
    let mut source =
        Handle::from_file(File::open(src).with_context(|| format!("opening {}", src.display()))?)?;
    let mut options = OpenOptions::new();
    options.write(true);
    if destination_meta.is_some() {
        options.create(false);
    } else {
        options.create_new(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options.mode(source_meta.permissions().mode() & 0o777);
    }
    #[cfg(not(unix))]
    let _ = source_meta;
    let mut destination = Handle::from_file(
        options
            .open(dst)
            .with_context(|| format!("opening {}", dst.display()))?,
    )?;
    // Compare opened handles before truncation, including Windows file IDs.
    if source == destination {
        bail!("cp source and destination refer to the same file");
    }
    destination.as_file().set_len(0)?;
    io::copy(source.as_file_mut(), destination.as_file_mut())
        .with_context(|| format!("copying {} to {}", src.display(), dst.display()))?;
    Ok(())
}
