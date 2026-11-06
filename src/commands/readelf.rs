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
struct ReadelfArgs {
    #[command(flatten)]
    _demangle: Demangle,
    #[arg(short = 'a', long = "all")]
    _all: bool,
    #[arg(short = 'h', long = "file-header")]
    _file_header: bool,
    #[arg(short = 'l', long = "program-headers", alias = "segments")]
    _program_headers: bool,
    #[arg(short = 'S', long = "section-headers", alias = "sections")]
    _section_headers: bool,
    #[arg(short = 'g', long = "section-groups")]
    _section_groups: bool,
    #[arg(short = 't', long = "section-details")]
    _section_details: bool,
    #[arg(short = 'e', long = "headers")]
    _headers: bool,
    #[arg(short = 's', long = "syms", alias = "symbols")]
    _syms: bool,
    #[arg(long = "dyn-syms")]
    _dyn_syms: bool,
    #[arg(long = "lto-syms")]
    _lto_syms: bool,
    #[arg(short = 'n', long = "notes")]
    _notes: bool,
    #[arg(short = 'r', long = "relocs")]
    _relocs: bool,
    #[arg(short = 'u', long = "unwind")]
    _unwind: bool,
    #[arg(short = 'd', long = "dynamic")]
    _dynamic: bool,
    #[arg(short = 'V', long = "version-info")]
    _version_info: bool,
    #[arg(short = 'A', long = "arch-specific")]
    _arch_specific: bool,
    #[arg(short = 'c', long = "archive-index")]
    _archive_index: bool,
    #[arg(short = 'D', long = "use-dynamic")]
    _use_dynamic: bool,
    #[arg(short = 'L', long = "lint", alias = "enable-checks")]
    _lint: bool,
    #[arg(short = 'x', long = "hex-dump")]
    _hex_dump: Vec<String>,
    #[arg(short = 'p', long = "string-dump")]
    _string_dump: Vec<String>,
    #[arg(short = 'R', long = "relocated-dump")]
    _relocated_dump: Vec<String>,
    #[arg(short = 'z', long = "decompress")]
    _decompress: bool,
    #[arg(short = 'I', long = "histogram")]
    _histogram: bool,
    #[arg(short = 'W', long = "wide")]
    _wide: bool,
    #[arg(short = 'T', long = "silent-truncation")]
    _silent_truncation: bool,
    files: Vec<String>,
}

fn effects(argv: &[String]) -> std::result::Result<Effects, String> {
    objects(parse_args::<ReadelfArgs>(argv)?.files, None)
}

#[derive(Debug)]
pub struct Readelf {
    pub data: JobData,
    pub argv: Vec<String>,
}

impl Readelf {
    pub fn parse(argv: &[String], _hints: &ShellHints) -> Result<Self> {
        Ok(Self {
            data: binary_common::job_data(effects(argv)),
            argv: argv.to_vec(),
        })
    }
}

impl Job for Readelf {
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
