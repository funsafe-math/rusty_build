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

use super::binary_common::{Effects, parse_args, require_stdin};
use clap::Parser;

#[derive(Parser)]
#[command(
    disable_help_flag = true,
    disable_version_flag = true,
    args_override_self = true
)]
struct FiltArgs {
    #[arg(short = '_', long = "strip-underscore")]
    _strip_underscore: bool,
    #[arg(short = 'n', long = "no-strip-underscore")]
    _no_strip_underscore: bool,
    #[arg(short = 'p', long = "no-params")]
    _no_params: bool,
    #[arg(short = 'i', long = "no-verbose")]
    _no_verbose: bool,
    #[arg(short = 'R', long = "recurse-limit")]
    _recurse_limit: bool,
    #[arg(short = 'r', long = "no-recurse-limit")]
    _no_recurse_limit: bool,
    #[arg(short = 't', long = "types")]
    _types: bool,
    #[arg(short = 's', long = "format", value_parser = ["none", "auto", "gnu-v3", "java", "gnat", "dlang", "rust"])]
    _format: Option<String>,
    symbols: Vec<String>,
}

fn effects(shell: &ShellCommand) -> std::result::Result<Effects, String> {
    let args = parse_args::<FiltArgs>(shell)?;
    if args.symbols.is_empty() {
        require_stdin(shell)?;
    }
    Ok(binary_common::known(Vec::new(), Vec::new()))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Cxxfilt {
    #[serde(flatten)]
    pub data: ToolData,
}

impl Job for Cxxfilt {
    const TYPE: &'static str = "c++filt";

    fn patterns() -> Vec<String> {
        vec![crate::registry::binary_pattern(r"(?:c\+\+filt|cxxfilt)")]
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
            program: "c++filt".into(),
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
        let result = effects(&command(&["_Z3foov", "not/a/file"], None)).unwrap();
        assert!(result.inputs.is_empty());
        assert!(result.outputs.is_empty());
    }

    #[test]
    fn hidden_inputs_and_unknown_arguments_fall_back_without_losing_argv() {
        let shell = command(&["--unknown", "symbol"], None);
        assert!(effects(&shell).is_err());
        let job = Cxxfilt::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
        let shell = command(&["@options"], None);
        assert!(effects(&shell).is_err());
        let job = Cxxfilt::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
        let shell = command(&["--", "@options"], None);
        assert!(effects(&shell).is_err());
        let job = Cxxfilt::parse(&shell).unwrap();
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
