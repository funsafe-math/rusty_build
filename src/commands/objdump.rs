use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::{
    binary_common::{self, ToolData},
    external::execute_argv,
};
use crate::{
    job::{BatchContext, BatchItem, BatchReport, ExecutionContext, Job, JobRecord, JobSpec},
    shell::ShellCommand,
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
    files: Vec<PathBuf>,
}

fn effects(shell: &ShellCommand) -> std::result::Result<Effects, String> {
    let args = parse_args::<ObjdumpArgs>(shell)?;
    // Explicit files avoid version-specific default/usage behavior.
    let mut effects = objects(args.files, None)?;
    effects.debug_lookup =
        (args.disassemble.is_some() || args.disassemble_all) && args.no_follow_links.is_none();
    Ok(effects)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Objdump {
    #[serde(flatten)]
    pub data: ToolData,
}

impl Job for Objdump {
    const TYPE: &'static str = "objdump";

    fn patterns() -> Vec<String> {
        vec![crate::registry::binary_pattern(r"objdump")]
    }

    fn parse(shell: &ShellCommand) -> Result<Self> {
        Ok(Self {
            data: ToolData::from_effects(shell, effects(shell)),
        })
    }

    fn inputs(&self) -> impl Iterator<Item = &Path> {
        self.data.inputs.iter().map(PathBuf::as_path)
    }

    fn outputs(&self) -> impl Iterator<Item = &Path> {
        self.data.outputs.iter().map(PathBuf::as_path)
    }

    fn unknown_effects(&self) -> bool {
        self.data.unknown_effects
    }

    fn refine_effects(&self, spec: &mut JobSpec, cwd: &Path) -> Result<()> {
        binary_common::refine_effects(spec, cwd, true)
    }

    fn validate(&self, record: &JobRecord, cwd: &Path) -> Result<()> {
        binary_common::validate(record, cwd, true)
    }

    fn execute(&self, ctx: &ExecutionContext<'_>) -> Result<()> {
        execute_argv(&self.data.argv, ctx)
    }

    fn execute_batch(items: &[BatchItem<'_, Self>], ctx: &BatchContext<'_>) -> BatchReport {
        // External tools have independent invocations; preserve each exact argv.
        ctx.run_each(items, |job, execution| job.execute(execution))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(args: &[&str], stdin: Option<&str>) -> ShellCommand {
        ShellCommand {
            program: "objdump".into(),
            args: args.iter().map(|arg| (*arg).into()).collect(),
            stdin: stdin.map(PathBuf::from),
            stdout: None,
            stderr: None,
            append_stdout: false,
        }
    }

    #[test]
    fn object_operands_exclude_option_values_and_positional_data() {
        let result = effects(&command(&["-h", "object.o"], None)).unwrap();
        assert_eq!(
            result.inputs,
            ["object.o"].iter().map(PathBuf::from).collect::<Vec<_>>()
        );
        assert!(result.outputs.is_empty());
        let result = effects(&command(&["-d", "object.o"], None)).unwrap();
        assert_eq!(
            result.inputs,
            ["object.o"].iter().map(PathBuf::from).collect::<Vec<_>>()
        );
        assert!(result.outputs.is_empty());
        let result = effects(&command(&["-at", "object.o"], None)).unwrap();
        assert_eq!(
            result.inputs,
            ["object.o"].iter().map(PathBuf::from).collect::<Vec<_>>()
        );
        assert!(result.outputs.is_empty());
        let result = effects(&command(
            &["--disassemble=symbol", "-j", ".text", "object.o"],
            None,
        ))
        .unwrap();
        assert_eq!(
            result.inputs,
            ["object.o"].iter().map(PathBuf::from).collect::<Vec<_>>()
        );
        assert!(result.outputs.is_empty());
    }

    #[test]
    fn hidden_inputs_and_unknown_arguments_fall_back_without_losing_argv() {
        let shell = command(&["-S", "object.o"], None);
        assert!(effects(&shell).is_err());
        let job = Objdump::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
        let shell = command(&["-l", "object.o"], None);
        assert!(effects(&shell).is_err());
        let job = Objdump::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
        let shell = command(&["--source-comment=text", "object.o"], None);
        assert!(effects(&shell).is_err());
        let job = Objdump::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
        let shell = command(&["--dwarf=follow-links", "object.o"], None);
        assert!(effects(&shell).is_err());
        let job = Objdump::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
        let shell = command(&["-L", "object.o"], None);
        assert!(effects(&shell).is_err());
        let job = Objdump::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
        let shell = command(&["@options"], None);
        assert!(effects(&shell).is_err());
        let job = Objdump::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
        let shell = command(&["--", "@options"], None);
        assert!(effects(&shell).is_err());
        let job = Objdump::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
    }

    #[test]
    fn accepted_disassembly_modes_guard_external_debug_lookup() {
        for (args, expected) in [
            (vec!["-h", "object.o"], false),
            (vec!["-d", "object.o"], true),
            (vec!["-D", "object.o"], true),
            (vec!["-d", "--dwarf=no-follow-links", "object.o"], false),
        ] {
            assert_eq!(
                effects(&command(&args, None)).unwrap().debug_lookup,
                expected
            );
        }
    }
}
