use anyhow::Result;

use super::{binary_common, external::execute_argv};
use crate::{
    job::{Job, JobData},
    shell::ShellHints,
};

use super::binary_common::{Effects, objects, parse_args};
use clap::Parser;

#[derive(Parser)]
#[command(
    disable_help_flag = true,
    disable_version_flag = true,
    args_override_self = true
)]
struct SizeArgs {
    #[arg(short = 'A')]
    _sysv: bool,
    #[arg(short = 'B')]
    _berkeley: bool,
    #[arg(short = 'G')]
    _gnu: bool,
    #[arg(long = "format", value_parser = ["sysv", "berkeley", "gnu"])]
    _format: Option<String>,
    #[arg(short = 'o')]
    _octal: bool,
    #[arg(short = 'd')]
    _decimal: bool,
    #[arg(short = 'x')]
    _hex: bool,
    #[arg(long = "radix", value_parser = ["8", "10", "16"])]
    _radix: Option<String>,
    #[arg(short = 't', long = "totals")]
    _totals: bool,
    #[arg(short = 'f')]
    _ignored: bool,
    #[arg(long = "common")]
    _common: bool,
    files: Vec<String>,
}

fn effects(argv: &[String]) -> std::result::Result<Effects, String> {
    objects(parse_args::<SizeArgs>(argv)?.files, Some("a.out"))
}

#[derive(Debug)]
pub struct Size {
    pub data: JobData,
    pub argv: Vec<String>,
}

impl Size {
    pub fn parse(argv: &[String], _hints: &ShellHints) -> Result<Self> {
        Ok(Self {
            data: binary_common::job_data(effects(argv)),
            argv: argv.to_vec(),
        })
    }
}

impl Job for Size {
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
