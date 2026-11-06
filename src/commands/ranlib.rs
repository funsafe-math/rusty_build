use anyhow::Result;

use super::{binary_common, external::execute_argv};
use crate::{
    job::{Job, JobData},
    shell::ShellHints,
};

use super::binary_common::{Effects, command, files, flag, inplace, parse, paths, short_flag};

fn effects(argv: &[String]) -> std::result::Result<Effects, String> {
    let matches = parse(
        command("ranlib")
            .arg(flag("enable-deterministic-archives", Some('D')))
            .arg(flag("disable-deterministic-archives", Some('U')))
            .arg(short_flag("touch", 't'))
            .arg(files(1, None)),
        argv,
    )?;
    Ok(inplace(paths(&matches)?))
}

#[derive(Debug)]
pub struct Ranlib {
    pub data: JobData,
    pub argv: Vec<String>,
}

impl Ranlib {
    pub fn parse(argv: &[String], _hints: &ShellHints) -> Result<Self> {
        Ok(Self {
            data: binary_common::job_data(effects(argv)),
            argv: argv.to_vec(),
        })
    }
}

impl Job for Ranlib {
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
