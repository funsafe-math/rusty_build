use anyhow::Result;

use super::{binary_common, external::execute_argv};
use crate::{
    job::{Job, JobData},
    shell::ShellHints,
};

use super::binary_common::{Effects, objects, parse_args};
use clap::Parser;

// Only the LLVM readobj forms below are supported. In particular this does not
// assume that GNU readelf's short options have LLVM readobj semantics.
#[derive(Parser)]
#[command(
    disable_help_flag = true,
    disable_version_flag = true,
    args_override_self = true
)]
struct ReadobjArgs {
    #[arg(long = "file-headers")]
    _file_headers: bool,
    #[arg(long = "sections")]
    _sections: bool,
    #[arg(long = "section-data")]
    _section_data: bool,
    #[arg(long = "section-relocations")]
    _section_relocations: bool,
    #[arg(long = "section-symbols")]
    _section_symbols: bool,
    #[arg(long = "symbols")]
    _symbols: bool,
    #[arg(long = "dyn-symbols")]
    _dyn_symbols: bool,
    #[arg(long = "relocations")]
    _relocations: bool,
    #[arg(long = "notes")]
    _notes: bool,
    #[arg(long = "program-headers")]
    _program_headers: bool,
    #[arg(long = "dynamic-table")]
    _dynamic_table: bool,
    #[arg(long = "needed-libs")]
    _needed_libs: bool,
    #[arg(long = "unwind")]
    _unwind: bool,
    #[arg(long = "elf-output-style", value_parser = ["LLVM", "GNU", "JSON"])]
    _elf_output_style: Option<String>,
    files: Vec<String>,
}

fn effects(argv: &[String]) -> std::result::Result<Effects, String> {
    objects(parse_args::<ReadobjArgs>(argv)?.files, None)
}

#[derive(Debug)]
pub struct Readobj {
    pub data: JobData,
    pub argv: Vec<String>,
}

impl Readobj {
    pub fn parse(argv: &[String], _hints: &ShellHints) -> Result<Self> {
        Ok(Self {
            data: binary_common::job_data(effects(argv)),
            argv: argv.to_vec(),
        })
    }
}

impl Job for Readobj {
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
