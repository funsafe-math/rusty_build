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
    files: Vec<PathBuf>,
}

fn effects(shell: &ShellCommand) -> std::result::Result<Effects, String> {
    let args = parse_args::<StringsArgs>(shell)?;
    if args.files.is_empty() {
        require_stdin(shell)?;
        Ok(binary_common::known(Vec::new(), Vec::new()))
    } else {
        objects(args.files, None)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Strings {
    #[serde(flatten)]
    pub data: ToolData,
}

impl Job for Strings {
    const TYPE: &'static str = "strings";

    fn patterns() -> Vec<String> {
        vec![crate::registry::binary_pattern(r"strings")]
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
        binary_common::refine_effects(spec, cwd, false)
    }

    fn validate(&self, record: &JobRecord, cwd: &Path) -> Result<()> {
        binary_common::validate(record, cwd, false)
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
            program: "strings".into(),
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
        let result = effects(&command(&["-n", "12", "object.o"], None)).unwrap();
        assert_eq!(
            result.inputs,
            ["object.o"].iter().map(PathBuf::from).collect::<Vec<_>>()
        );
        assert!(result.outputs.is_empty());
        let result = effects(&command(&["-s", "separator", "object.o"], None)).unwrap();
        assert_eq!(
            result.inputs,
            ["object.o"].iter().map(PathBuf::from).collect::<Vec<_>>()
        );
        assert!(result.outputs.is_empty());
    }

    #[test]
    fn hidden_inputs_and_unknown_arguments_fall_back_without_losing_argv() {
        let shell = command(&["--target=plugin", "object.o"], None);
        assert!(effects(&shell).is_err());
        let job = Strings::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
        let shell = command(&["-"], None);
        assert!(effects(&shell).is_err());
        let job = Strings::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
        let shell = command(&["@options"], None);
        assert!(effects(&shell).is_err());
        let job = Strings::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
        let shell = command(&["--", "@options"], None);
        assert!(effects(&shell).is_err());
        let job = Strings::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
    }

    #[test]
    fn stdin_requires_explicit_redirection_without_claiming_its_effects() {
        assert!(effects(&command(&[], None)).is_err());
        let result = effects(&command(&[], Some("names"))).unwrap();
        assert!(result.inputs.is_empty());
        assert!(result.outputs.is_empty());
    }
}
