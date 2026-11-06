use anyhow::Result;

use super::{binary_common, external::execute_argv};
use crate::{
    job::{Job, JobData},
    shell::ShellHints,
};

use super::binary_common::{Effects, objects, parse_args, require_stdin};
use clap::Parser;

#[derive(Parser)]
#[command(
    disable_help_flag = true,
    disable_version_flag = true,
    args_override_self = true
)]
struct Addr2lineArgs {
    #[arg(short = 'e', long = "exe", default_value = "a.out")]
    exe: String,
    #[arg(short = 'a', long = "addresses")]
    _addresses: bool,
    #[arg(short = 'i', long = "inlines")]
    _inlines: bool,
    #[arg(short = 'j', long = "section")]
    _section: Option<String>,
    #[arg(short = 'p', long = "pretty-print")]
    _pretty: bool,
    #[arg(short = 's', long = "basenames")]
    _basenames: bool,
    #[arg(short = 'f', long = "functions")]
    _functions: bool,
    #[arg(short = 'C', long = "demangle", num_args = 0..=1, require_equals = true, default_missing_value = "auto")]
    _demangle: Option<String>,
    #[arg(short = 'R', long = "recurse-limit")]
    _recurse_limit: bool,
    #[arg(short = 'r', long = "no-recurse-limit")]
    _no_recurse_limit: bool,
    addresses: Vec<String>,
}

fn effects(argv: &[String]) -> std::result::Result<Effects, String> {
    let args = parse_args::<Addr2lineArgs>(argv)?;
    if args.addresses.is_empty() {
        require_stdin(argv)?;
    }
    let mut effects = objects(vec![args.exe], None)?;
    effects.debug_lookup = true;
    Ok(effects)
}

#[derive(Debug)]
pub struct Addr2line {
    pub data: JobData,
    pub argv: Vec<String>,
}

impl Addr2line {
    pub fn parse(argv: &[String], _hints: &ShellHints) -> Result<Self> {
        Ok(Self {
            data: binary_common::job_data(effects(argv)),
            argv: argv.to_vec(),
        })
    }
}

impl Job for Addr2line {
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
