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

use super::binary_common::{
    Effects, command, copy_options, files, flag, inplace, known, last_path, parse, paths,
    short_flag, value,
};

fn effects(shell: &ShellCommand) -> std::result::Result<Effects, String> {
    let matches = parse(
        copy_options(command("strip"))
            .arg(flag("strip-debug", Some('g')))
            .arg(short_flag("debug-short-S", 'S'))
            .arg(short_flag("debug-short-d", 'd'))
            .arg(value("output-file", Some('o')))
            .arg(files(1, None)),
        shell,
    )?;
    let files = paths(&matches)?;
    match last_path(&matches, "output-file")? {
        Some(output) if files.len() == 1 => Ok(known(files, vec![output])),
        Some(_) => {
            Err("strip -o with multiple inputs has no single understood output mapping".into())
        }
        None => Ok(inplace(files)),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Strip {
    #[serde(flatten)]
    pub data: ToolData,
}

impl Job for Strip {
    const TYPE: &'static str = "strip";

    fn patterns() -> Vec<String> {
        vec![crate::registry::binary_pattern(r"strip")]
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

    fn infer(text: &str) -> std::result::Result<Effects, String> {
        let shell = crate::shell::parse(text).unwrap().unwrap();
        effects(&shell)
    }

    fn paths_of(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn strip_output_and_inplace_hazards() {
        let effect = infer("strip --strip-debug first.o second.o").unwrap();
        assert_eq!(effect.inputs, paths_of(&["first.o", "second.o"]));
        assert_eq!(effect.outputs, effect.inputs);
        let effect = infer("strip --strip-debug -o stripped.o input.o").unwrap();
        assert_eq!(effect.inputs, paths_of(&["input.o"]));
        assert_eq!(effect.outputs, paths_of(&["stripped.o"]));
        assert!(infer("strip -o out.o a.o b.o").is_err());
    }

    #[test]
    fn opaque_arguments_request_fallback_without_losing_argv() {
        assert!(infer("strip --plugin=plugin.so input.o").is_err());
        let shell = crate::shell::parse("strip --plugin=plugin.so input.o")
            .unwrap()
            .unwrap();
        let job = Strip::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
    }
}
