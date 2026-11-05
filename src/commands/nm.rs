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
    files: Vec<PathBuf>,
}

fn effects(shell: &ShellCommand) -> std::result::Result<Effects, String> {
    objects(parse_args::<NmArgs>(shell)?.files, Some("a.out"))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Nm {
    #[serde(flatten)]
    pub data: ToolData,
}

impl Job for Nm {
    const TYPE: &'static str = "nm";

    fn patterns() -> Vec<String> {
        vec![crate::registry::binary_pattern(r"nm")]
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
            program: "nm".into(),
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
        let result = effects(&command(
            &["-n", "--defined-only", "first.o", "second.o"],
            None,
        ))
        .unwrap();
        assert_eq!(
            result.inputs,
            ["first.o", "second.o"]
                .iter()
                .map(PathBuf::from)
                .collect::<Vec<_>>()
        );
        assert!(result.outputs.is_empty());
        let result = effects(&command(&["-C", "--format=posix", "object.o"], None)).unwrap();
        assert_eq!(
            result.inputs,
            ["object.o"].iter().map(PathBuf::from).collect::<Vec<_>>()
        );
        assert!(result.outputs.is_empty());
        let result = effects(&command(&[], None)).unwrap();
        assert_eq!(
            result.inputs,
            ["a.out"].iter().map(PathBuf::from).collect::<Vec<_>>()
        );
        assert!(result.outputs.is_empty());
        let result = effects(&command(&["--", "-object.o"], None)).unwrap();
        assert_eq!(
            result.inputs,
            ["-object.o"].iter().map(PathBuf::from).collect::<Vec<_>>()
        );
        assert!(result.outputs.is_empty());
        let result = effects(&command(&["-a", "object.o"], None)).unwrap();
        assert_eq!(
            result.inputs,
            ["object.o"].iter().map(PathBuf::from).collect::<Vec<_>>()
        );
        assert!(result.outputs.is_empty());
    }

    #[test]
    fn hidden_inputs_and_unknown_arguments_fall_back_without_losing_argv() {
        let shell = command(&["--plugin=plugin.so", "object.o"], None);
        assert!(effects(&shell).is_err());
        let job = Nm::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
        let shell = command(&["-l", "object.o"], None);
        assert!(effects(&shell).is_err());
        let job = Nm::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
        let shell = command(&["--unknown", "object.o"], None);
        assert!(effects(&shell).is_err());
        let job = Nm::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
        let shell = command(&["@options"], None);
        assert!(effects(&shell).is_err());
        let job = Nm::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
        let shell = command(&["--", "@options"], None);
        assert!(effects(&shell).is_err());
        let job = Nm::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
    }

    #[test]
    fn ordinary_symbol_inspection_does_not_discover_debug_files() {
        assert!(
            !effects(&command(&["-a", "object.o"], None))
                .unwrap()
                .debug_lookup
        );
    }
}
