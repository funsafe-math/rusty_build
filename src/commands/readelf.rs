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
    files: Vec<PathBuf>,
}

fn effects(shell: &ShellCommand) -> std::result::Result<Effects, String> {
    objects(parse_args::<ReadelfArgs>(shell)?.files, None)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Readelf {
    #[serde(flatten)]
    pub data: ToolData,
}

impl Job for Readelf {
    const TYPE: &'static str = "readelf";

    fn patterns() -> Vec<String> {
        vec![crate::registry::binary_pattern(r"readelf")]
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
            program: "readelf".into(),
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
        let result = effects(&command(&["-a", "object.o"], None)).unwrap();
        assert_eq!(
            result.inputs,
            ["object.o"].iter().map(PathBuf::from).collect::<Vec<_>>()
        );
        assert!(result.outputs.is_empty());
        let result = effects(&command(&["-p", ".comment", "object.o"], None)).unwrap();
        assert_eq!(
            result.inputs,
            ["object.o"].iter().map(PathBuf::from).collect::<Vec<_>>()
        );
        assert!(result.outputs.is_empty());
    }

    #[test]
    fn hidden_inputs_and_unknown_arguments_fall_back_without_losing_argv() {
        let shell = command(&["-wK", "object.o"], None);
        assert!(effects(&shell).is_err());
        let job = Readelf::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
        let shell = command(&["--process-links", "object.o"], None);
        assert!(effects(&shell).is_err());
        let job = Readelf::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
        let shell = command(&["-h"], None);
        assert!(effects(&shell).is_err());
        let job = Readelf::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
        let shell = command(&["@options"], None);
        assert!(effects(&shell).is_err());
        let job = Readelf::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
        let shell = command(&["--", "@options"], None);
        assert!(effects(&shell).is_err());
        let job = Readelf::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
    }
}
