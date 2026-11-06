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
struct StringsArgs {
    #[arg(short = 'a', long = "all")]
    _all: bool,
    #[arg(short = 'd', long = "data")]
    _data: bool,
    #[arg(short = 'f', long = "print-file-name")]
    _print_file_name: bool,
    #[arg(short = 'n', long = "bytes")]
    _bytes: Option<usize>,
    #[arg(short = 't', long = "radix", value_parser = ["o", "d", "x"])]
    _radix: Option<String>,
    #[arg(short = 'w', long = "include-all-whitespace")]
    _whitespace: bool,
    #[arg(short = 'o')]
    _octal: bool,
    #[arg(short = 'e', long = "encoding", value_parser = ["s", "S", "b", "l", "B", "L"])]
    _encoding: Option<String>,
    #[arg(short = 'U', long = "unicode")]
    _unicode: Option<String>,
    #[arg(short = 's', long = "output-separator")]
    _separator: Option<String>,
    files: Vec<String>,
}

fn effects(argv: &[String]) -> std::result::Result<Effects, String> {
    let args = parse_args::<StringsArgs>(argv)?;
    if args.files.is_empty() {
        require_stdin(argv)?;
        Ok(binary_common::known(Vec::new(), Vec::new()))
    } else {
        objects(args.files, None)
    }
}

#[derive(Debug)]
pub struct Strings {
    pub data: JobData,
    pub argv: Vec<String>,
}

impl Strings {
    pub fn parse(argv: &[String], _hints: &ShellHints) -> Result<Self> {
        Ok(Self {
            data: binary_common::job_data(effects(argv)),
            argv: argv.to_vec(),
        })
    }
}

impl Job for Strings {
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
