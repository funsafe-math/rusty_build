use std::fs::{self, FileTimes, OpenOptions};
use std::io;
use std::path::Path;

use anyhow::{Context, Result};
use clap::Parser;

use crate::job::{FileAccess, Job, JobData};
use crate::shell::ShellHints;

use super::external::execute_native;
use super::fs::{checked_regular, reject_symlink_components};

#[derive(Debug, Parser)]
#[command(disable_help_flag = true, disable_version_flag = true)]
pub struct Touch {
    #[arg(required = true, num_args = 1..)]
    pub paths: Vec<String>,
    #[arg(skip)]
    pub data: JobData,
}

impl Touch {
    pub fn parse(argv: &[String], shell: &ShellHints) -> Result<Self> {
        let mut job = Self::try_parse_from(argv)?;
        // Touch can create a missing file; it has no required content input.
        job.data = JobData::new(job.paths.iter().map(FileAccess::write).collect());
        job.data.shell = shell.clone();
        Ok(job)
    }
}

impl Job for Touch {
    fn data(&self) -> &JobData {
        &self.data
    }

    fn data_mut(&mut self) -> &mut JobData {
        &mut self.data
    }

    fn execute(&self) -> Result<()> {
        execute_native(&self.data.shell, || {
            reject_symlink_components(self.paths.iter().map(Path::new))?;
            for path in &self.paths {
                touch(Path::new(path))?;
            }
            Ok(())
        })
    }
}

fn touch(path: &Path) -> Result<()> {
    let file = match fs::symlink_metadata(path) {
        Ok(_) => {
            checked_regular(path)?;
            // Windows needs write access for SetFileTime; Unix can also touch
            // an owned read-only file through a read-only descriptor.
            let mut options = OpenOptions::new();
            options.read(true);
            #[cfg(windows)]
            options.write(true);
            options
                .open(path)
                .with_context(|| format!("opening {} for touch", path.display()))?
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .with_context(|| format!("creating {} for touch", path.display()))?,
        Err(error) => return Err(error).with_context(|| format!("inspecting {}", path.display())),
    };
    let now = std::time::SystemTime::now();
    file.set_times(FileTimes::new().set_accessed(now).set_modified(now))
        .with_context(|| format!("touch {}", path.display()))?;
    Ok(())
}
