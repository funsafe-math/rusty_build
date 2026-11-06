use anyhow::Result;

use super::{binary_common, external::execute_argv};
use crate::{
    job::{Job, JobData},
    shell::ShellHints,
};

use super::binary_common::{Demangle, Effects, objects, parse_args};
use clap::Parser;

#[derive(Parser)]
#[command(
    disable_help_flag = true,
    disable_version_flag = true,
    args_override_self = true
)]
struct NmArgs {
    #[command(flatten)]
    _demangle: Demangle,
    #[arg(short = 'a', long = "debug-syms")]
    _debug_syms: bool,
    #[arg(short = 'A', long = "print-file-name", short_alias = 'o')]
    _print_file_name: bool,
    #[arg(short = 'B')]
    _bsd: bool,
    #[arg(short = 'D', long = "dynamic")]
    _dynamic: bool,
    #[arg(short = 'e')]
    _ignored: bool,
    #[arg(short = 'f', long = "format", value_parser = ["bsd", "sysv", "posix", "just-symbols"])]
    _format: Option<String>,
    #[arg(short = 'g', long = "extern-only")]
    _extern_only: bool,
    #[arg(short = 'j', long = "just-symbols")]
    _just_symbols: bool,
    #[arg(short = 'n', long = "numeric-sort", short_alias = 'v')]
    _numeric_sort: bool,
    #[arg(short = 'p', long = "no-sort")]
    _no_sort: bool,
    #[arg(short = 'P', long = "portability")]
    _portability: bool,
    #[arg(short = 'r', long = "reverse-sort")]
    _reverse_sort: bool,
    #[arg(short = 'S', long = "print-size")]
    _print_size: bool,
    #[arg(short = 's', long = "print-armap")]
    _print_armap: bool,
    #[arg(short = 't', long = "radix", value_parser = ["d", "o", "x"])]
    _radix: Option<String>,
    #[arg(short = 'u', long = "undefined-only")]
    _undefined_only: bool,
    #[arg(short = 'U', long = "defined-only")]
    _defined_only: bool,
    #[arg(short = 'W', long = "no-weak")]
    _no_weak: bool,
    #[arg(long = "quiet")]
    _quiet: bool,
    #[arg(long = "size-sort")]
    _size_sort: bool,
    #[arg(long = "special-syms")]
    _special_syms: bool,
    #[arg(long = "synthetic")]
    _synthetic: bool,
    #[arg(long = "without-symbol-versions")]
    _without_versions: bool,
    files: Vec<String>,
}

fn effects(argv: &[String]) -> std::result::Result<Effects, String> {
    objects(parse_args::<NmArgs>(argv)?.files, Some("a.out"))
}

#[derive(Debug)]
pub struct Nm {
    pub data: JobData,
    pub argv: Vec<String>,
}

impl Nm {
    pub fn parse(argv: &[String], _hints: &ShellHints) -> Result<Self> {
        Ok(Self {
            data: binary_common::job_data(effects(argv)),
            argv: argv.to_vec(),
        })
    }
}

impl Job for Nm {
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
