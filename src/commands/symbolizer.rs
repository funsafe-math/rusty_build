use anyhow::Result;

use super::{binary_common, external::execute_argv};
use crate::{
    job::{Job, JobData},
    shell::ShellHints,
};

use super::binary_common::Effects;

// Argv alone cannot close this tool's read set; keep every option opaque.
fn effects(_argv: &[String]) -> std::result::Result<Effects, String> {
    Err("symbolizer may discover source/debug files; complete effects are not modeled".into())
}

#[derive(Debug)]
pub struct Symbolizer {
    pub data: JobData,
    pub argv: Vec<String>,
}

impl Symbolizer {
    pub fn parse(argv: &[String], _hints: &ShellHints) -> Result<Self> {
        Ok(Self {
            data: binary_common::job_data(effects(argv)),
            argv: argv.to_vec(),
        })
    }
}

impl Job for Symbolizer {
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
