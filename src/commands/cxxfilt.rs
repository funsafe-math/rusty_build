use anyhow::Result;

use super::{binary_common, external::execute_argv};
use crate::{
    job::{Job, JobData},
    shell::ShellHints,
};

use super::binary_common::{Effects, parse_args, require_stdin};
use clap::Parser;

#[derive(Parser)]
#[command(
    disable_help_flag = true,
    disable_version_flag = true,
    args_override_self = true
)]
struct FiltArgs {
    #[arg(short = '_', long = "strip-underscore")]
    _strip_underscore: bool,
    #[arg(short = 'n', long = "no-strip-underscore")]
    _no_strip_underscore: bool,
    #[arg(short = 'p', long = "no-params")]
    _no_params: bool,
    #[arg(short = 'i', long = "no-verbose")]
    _no_verbose: bool,
    #[arg(short = 'R', long = "recurse-limit")]
    _recurse_limit: bool,
    #[arg(short = 'r', long = "no-recurse-limit")]
    _no_recurse_limit: bool,
    #[arg(short = 't', long = "types")]
    _types: bool,
    #[arg(short = 's', long = "format", value_parser = ["none", "auto", "gnu-v3", "java", "gnat", "dlang", "rust"])]
    _format: Option<String>,
    symbols: Vec<String>,
}

fn effects(argv: &[String]) -> std::result::Result<Effects, String> {
    let args = parse_args::<FiltArgs>(argv)?;
    if args.symbols.is_empty() {
        require_stdin(argv)?;
    }
    Ok(binary_common::known(Vec::new(), Vec::new()))
}

#[derive(Debug)]
pub struct Cxxfilt {
    pub data: JobData,
    pub argv: Vec<String>,
}

impl Cxxfilt {
    pub fn parse(argv: &[String], _hints: &ShellHints) -> Result<Self> {
        Ok(Self {
            data: binary_common::job_data(effects(argv)),
            argv: argv.to_vec(),
        })
    }
}

impl Job for Cxxfilt {
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
