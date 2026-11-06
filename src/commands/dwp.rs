use anyhow::Result;

use super::{binary_common, external::execute_argv};
use crate::{
    job::{Job, JobData},
    shell::ShellHints,
};

use super::binary_common::{Effects, command, files, flag, known, last_path, parse, paths, value};

fn effects(argv: &[String]) -> std::result::Result<Effects, String> {
    // -e discovers .dwo inputs through an executable's debug information;
    // explicit .dwo inputs and an explicit output do not require that lookup.
    let matches = parse(
        command("dwp")
            .arg(value("output", Some('o')).required(true))
            .arg(flag("verbose", Some('v')))
            .arg(files(1, None)),
        argv,
    )?;
    let output = last_path(&matches, "output")?.expect("required output");
    Ok(known(paths(&matches)?, vec![output]))
}

#[derive(Debug)]
pub struct Dwp {
    pub data: JobData,
    pub argv: Vec<String>,
}

impl Dwp {
    pub fn parse(argv: &[String], _hints: &ShellHints) -> Result<Self> {
        Ok(Self {
            data: binary_common::job_data(effects(argv)),
            argv: argv.to_vec(),
        })
    }
}

impl Job for Dwp {
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
