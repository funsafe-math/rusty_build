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

use super::binary_common::{Effects, command, files, flag, inplace, parse, paths, short_flag};

fn effects(shell: &ShellCommand) -> std::result::Result<Effects, String> {
    let matches = parse(
        command("ranlib")
            .arg(flag("enable-deterministic-archives", Some('D')))
            .arg(flag("disable-deterministic-archives", Some('U')))
            .arg(short_flag("touch", 't'))
            .arg(files(1, None)),
        shell,
    )?;
    Ok(inplace(paths(&matches)?))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Ranlib {
    #[serde(flatten)]
    pub data: ToolData,
}

impl Job for Ranlib {
    const TYPE: &'static str = "ranlib";

    fn patterns() -> Vec<String> {
        vec![crate::registry::binary_pattern(r"ranlib")]
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
    fn every_archive_is_read_and_rewritten() {
        let effect = infer("ranlib first.a second.a").unwrap();
        assert_eq!(effect.inputs, paths_of(&["first.a", "second.a"]));
        assert_eq!(effect.outputs, effect.inputs);
    }

    #[test]
    fn opaque_arguments_request_fallback_without_losing_argv() {
        assert!(infer("ranlib --plugin=plugin.so lib.a").is_err());
        let shell = crate::shell::parse("ranlib --plugin=plugin.so lib.a")
            .unwrap()
            .unwrap();
        let job = Ranlib::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
    }
}
