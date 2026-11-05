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
struct Addr2lineArgs {
    #[arg(short = 'e', long = "exe", default_value = "a.out")]
    exe: PathBuf,
    #[arg(short = 'a', long = "addresses")]
    _addresses: bool,
    #[arg(short = 'i', long = "inlines")]
    _inlines: bool,
    #[arg(short = 'j', long = "section")]
    _section: Option<String>,
    #[arg(short = 'p', long = "pretty-print")]
    _pretty: bool,
    #[arg(short = 's', long = "basenames")]
    _basenames: bool,
    #[arg(short = 'f', long = "functions")]
    _functions: bool,
    #[arg(short = 'C', long = "demangle", num_args = 0..=1, require_equals = true, default_missing_value = "auto")]
    _demangle: Option<String>,
    #[arg(short = 'R', long = "recurse-limit")]
    _recurse_limit: bool,
    #[arg(short = 'r', long = "no-recurse-limit")]
    _no_recurse_limit: bool,
    addresses: Vec<String>,
}

fn effects(shell: &ShellCommand) -> std::result::Result<Effects, String> {
    let args = parse_args::<Addr2lineArgs>(shell)?;
    if args.addresses.is_empty() {
        require_stdin(shell)?;
    }
    let mut effects = objects(vec![args.exe], None)?;
    effects.debug_lookup = true;
    Ok(effects)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Addr2line {
    #[serde(flatten)]
    pub data: ToolData,
}

impl Job for Addr2line {
    const TYPE: &'static str = "addr2line";

    fn patterns() -> Vec<String> {
        vec![crate::registry::binary_pattern(r"addr2line")]
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
            program: "addr2line".into(),
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
            &[
                "-f", "-C", "-i", "-p", "-a", "-s", "-j", ".text", "-e", "app", "0", "0x1234",
            ],
            None,
        ))
        .unwrap();
        assert_eq!(
            result.inputs,
            ["app"].iter().map(PathBuf::from).collect::<Vec<_>>()
        );
        assert!(result.outputs.is_empty());
        let result = effects(&command(&["--exe=app", "0"], None)).unwrap();
        assert_eq!(
            result.inputs,
            ["app"].iter().map(PathBuf::from).collect::<Vec<_>>()
        );
        assert!(result.outputs.is_empty());
        let result = effects(&command(&["0"], None)).unwrap();
        assert_eq!(
            result.inputs,
            ["a.out"].iter().map(PathBuf::from).collect::<Vec<_>>()
        );
        assert!(result.outputs.is_empty());
    }

    #[test]
    fn hidden_inputs_and_unknown_arguments_fall_back_without_losing_argv() {
        let shell = command(&["--plugin=plugin.so", "0"], None);
        assert!(effects(&shell).is_err());
        let job = Addr2line::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
        let shell = command(&["@options"], None);
        assert!(effects(&shell).is_err());
        let job = Addr2line::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
        let shell = command(&["--", "@options"], None);
        assert!(effects(&shell).is_err());
        let job = Addr2line::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
    }

    #[test]
    fn stdin_requires_explicit_redirection_without_claiming_its_effects() {
        assert!(effects(&command(&[], None)).is_err());
        let result = effects(&command(&[], Some("names"))).unwrap();
        assert_eq!(
            result.inputs,
            ["a.out"].iter().map(PathBuf::from).collect::<Vec<_>>()
        );
        assert!(result.outputs.is_empty());
    }

    #[test]
    fn address_lookup_retains_external_debug_barrier() {
        assert!(
            effects(&command(&["-e", "object.o", "0"], None))
                .unwrap()
                .debug_lookup
        );
    }
}
