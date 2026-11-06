use anyhow::Result;

use super::{binary_common, external::execute_argv};
use crate::{
    job::{Job, JobData},
    shell::ShellHints,
};

use super::binary_common::{Effects, command, files, inplace, parse, paths, value};

fn effects(argv: &[String]) -> std::result::Result<Effects, String> {
    let mut parser = command("elfedit").arg(files(1, None));
    for name in [
        "input-mach",
        "output-mach",
        "input-type",
        "output-type",
        "input-osabi",
        "output-osabi",
        "input-abiversion",
        "output-abiversion",
    ] {
        parser = parser.arg(value(name, None));
    }
    let matches = parse(parser, argv)?;
    Ok(inplace(paths(&matches)?))
}

#[derive(Debug)]
pub struct Elfedit {
    pub data: JobData,
    pub argv: Vec<String>,
}

impl Elfedit {
    pub fn parse(argv: &[String], _hints: &ShellHints) -> Result<Self> {
        Ok(Self {
            data: binary_common::job_data(effects(argv)),
            argv: argv.to_vec(),
        })
    }
}

impl Job for Elfedit {
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
