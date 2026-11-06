use anyhow::Result;

use super::{binary_common, external::execute_argv};
use crate::{
    job::{Job, JobData},
    shell::ShellHints,
};

use super::binary_common::{
    Effects, command, copy_options, files, flag, inplace, known, last_path, parse, paths,
    short_flag, value,
};

fn effects(argv: &[String]) -> std::result::Result<Effects, String> {
    let matches = parse(
        copy_options(command("strip"))
            .arg(flag("strip-debug", Some('g')))
            .arg(short_flag("debug-short-S", 'S'))
            .arg(short_flag("debug-short-d", 'd'))
            .arg(value("output-file", Some('o')))
            .arg(files(1, None)),
        argv,
    )?;
    let files = paths(&matches)?;
    match last_path(&matches, "output-file")? {
        Some(output) if files.len() == 1 => Ok(known(files, vec![output])),
        Some(_) => {
            Err("strip -o with multiple inputs has no single understood output mapping".into())
        }
        None => Ok(inplace(files)),
    }
}

#[derive(Debug)]
pub struct Strip {
    pub data: JobData,
    pub argv: Vec<String>,
}

impl Strip {
    pub fn parse(argv: &[String], _hints: &ShellHints) -> Result<Self> {
        Ok(Self {
            data: binary_common::job_data(effects(argv)),
            argv: argv.to_vec(),
        })
    }
}

impl Job for Strip {
    fn data(&self) -> &JobData {
        &self.data
    }
    fn data_mut(&mut self) -> &mut JobData {
        &mut self.data
    }
    fn execute(&self) -> Result<()> {
        execute_argv(&self.argv, &self.data.shell)
    }
}
