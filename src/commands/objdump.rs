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
struct ObjdumpArgs {
    #[command(flatten)]
    _demangle: Demangle,
    #[arg(short = 'a', long = "archive-headers")]
    _archive_headers: bool,
    #[arg(short = 'f', long = "file-headers")]
    _file_headers: bool,
    #[arg(short = 'p', long = "private-headers")]
    _private_headers: bool,
    #[arg(short = 'h', long = "section-headers", alias = "headers")]
    _section_headers: bool,
    #[arg(short = 'x', long = "all-headers")]
    _all_headers: bool,
    #[arg(short = 'd', long = "disassemble", num_args = 0..=1, require_equals = true, default_missing_value = "")]
    disassemble: Option<String>,
    #[arg(short = 'D', long = "disassemble-all")]
    disassemble_all: bool,
    // This is a lookup-control flag, not permission to parse arbitrary DWARF
    // selectors. Source/line-number modes are still rejected independently.
    #[arg(long = "dwarf", value_parser = ["no-follow-links"], require_equals = true)]
    no_follow_links: Option<String>,
    #[arg(short = 's', long = "full-contents")]
    _full_contents: bool,
    #[arg(short = 'Z', long = "decompress")]
    _decompress: bool,
    #[arg(short = 't', long = "syms")]
    _syms: bool,
    #[arg(short = 'T', long = "dynamic-syms")]
    _dynamic_syms: bool,
    #[arg(short = 'r', long = "reloc")]
    _reloc: bool,
    #[arg(short = 'R', long = "dynamic-reloc")]
    _dynamic_reloc: bool,
    #[arg(short = 'j', long = "section")]
    _sections: Vec<String>,
    #[arg(short = 'm', long = "architecture")]
    _architecture: Option<String>,
    #[arg(short = 'w', long = "wide")]
    _wide: bool,
    #[arg(short = 'z', long = "disassemble-zeroes")]
    _zeroes: bool,
    #[arg(short = 'F', long = "file-offsets")]
    _file_offsets: bool,
    #[arg(long = "start-address")]
    _start: Option<String>,
    #[arg(long = "stop-address")]
    _stop: Option<String>,
    #[arg(long = "adjust-vma")]
    _adjust_vma: Option<String>,
    #[arg(long = "no-addresses")]
    _no_addresses: bool,
    #[arg(long = "prefix-addresses")]
    _prefix_addresses: bool,
    #[arg(long = "show-raw-insn")]
    _raw: bool,
    #[arg(long = "no-show-raw-insn")]
    _no_raw: bool,
    #[arg(long = "insn-width")]
    _insn_width: Option<usize>,
    #[arg(long = "show-all-symbols")]
    _show_all_symbols: bool,
    #[arg(long = "special-syms")]
    _special_syms: bool,
    files: Vec<String>,
}

fn effects(argv: &[String]) -> std::result::Result<Effects, String> {
    let args = parse_args::<ObjdumpArgs>(argv)?;
    // Explicit files avoid version-specific default/usage behavior.
    let mut effects = objects(args.files, None)?;
    effects.debug_lookup =
        (args.disassemble.is_some() || args.disassemble_all) && args.no_follow_links.is_none();
    Ok(effects)
}

#[derive(Debug)]
pub struct Objdump {
    pub data: JobData,
    pub argv: Vec<String>,
}

impl Objdump {
    pub fn parse(argv: &[String], _hints: &ShellHints) -> Result<Self> {
        Ok(Self {
            data: binary_common::job_data(effects(argv)),
            argv: argv.to_vec(),
        })
    }
}

impl Job for Objdump {
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
