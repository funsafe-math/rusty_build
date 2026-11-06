use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use clap::Parser;

use crate::job::{FileAccess, Job, JobData};
use crate::shell::ShellHints;

use super::external::execute_native;
use super::fs::reject_symlink_components;

#[derive(Debug, Parser)]
#[command(disable_help_flag = true, disable_version_flag = true)]
pub struct Mkdir {
    #[arg(short = 'p', long)]
    pub parents: bool,
    #[arg(required = true, num_args = 1..)]
    pub paths: Vec<String>,
    #[arg(skip)]
    pub data: JobData,
}

impl Mkdir {
    pub fn parse(argv: &[String], shell: &ShellHints) -> Result<Self> {
        let mut job = Self::try_parse_from(argv)?;
        let mut accesses = Vec::new();
        for path in &job.paths {
            accesses.push(FileAccess::write(path.clone()));
            if job.parents {
                // These prefixes describe parent effects for comparison only;
                // execution still receives the exact original operand.
                for (index, character) in path.char_indices() {
                    if (character == '/' || (cfg!(windows) && character == '\\')) && index > 0 {
                        let parent = &path[..index];
                        if !parent.ends_with(':')
                            && !parent.ends_with('/')
                            && !(cfg!(windows) && parent.ends_with('\\'))
                            && !accesses.iter().any(|access| access.path == parent)
                        {
                            accesses.push(FileAccess::write(parent));
                        }
                    }
                }
            }
        }
        job.data = JobData::new(accesses);
        job.data.shell = shell.clone();
        Ok(job)
    }
}

impl Job for Mkdir {
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
                if self.parents {
                    fs::create_dir_all(path).with_context(|| format!("mkdir -p {path}"))?;
                } else {
                    fs::create_dir(path).with_context(|| format!("mkdir {path}"))?;
                }
            }
            Ok(())
        })
    }
}
