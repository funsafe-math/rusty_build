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
    files: Vec<PathBuf>,
}

fn effects(shell: &ShellCommand) -> std::result::Result<Effects, String> {
    objects(parse_args::<ReadobjArgs>(shell)?.files, None)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Readobj {
    #[serde(flatten)]
    pub data: ToolData,
}

impl Job for Readobj {
    const TYPE: &'static str = "readobj";

    fn patterns() -> Vec<String> {
        vec![crate::registry::binary_pattern(r"readobj")]
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
            program: "readobj".into(),
            args: args.iter().map(|arg| (*arg).into()).collect(),
            stdin: stdin.map(PathBuf::from),
            stdout: None,
            stderr: None,
            append_stdout: false,
            noop_fallback: false,
        }
    }

    #[test]
    fn object_operands_exclude_option_values_and_positional_data() {
        let result = effects(&command(&["--file-headers", "object.o"], None)).unwrap();
        assert_eq!(
            result.inputs,
            ["object.o"].iter().map(PathBuf::from).collect::<Vec<_>>()
        );
        assert!(result.outputs.is_empty());
    }

    #[test]
    fn hidden_inputs_and_unknown_arguments_fall_back_without_losing_argv() {
        let shell = command(&["--unknown", "object.o"], None);
        assert!(effects(&shell).is_err());
        let job = Readobj::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
        let shell = command(&["@options"], None);
        assert!(effects(&shell).is_err());
        let job = Readobj::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
        let shell = command(&["--", "@options"], None);
        assert!(effects(&shell).is_err());
        let job = Readobj::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
    }
}
