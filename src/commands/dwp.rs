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

use super::binary_common::{Effects, command, files, flag, known, last_path, parse, paths, value};

fn effects(shell: &ShellCommand) -> std::result::Result<Effects, String> {
    // -e discovers .dwo inputs through an executable's debug information;
    // explicit .dwo inputs and an explicit output do not require that lookup.
    let matches = parse(
        command("dwp")
            .arg(value("output", Some('o')).required(true))
            .arg(flag("verbose", Some('v')))
            .arg(files(1, None)),
        shell,
    )?;
    let output = last_path(&matches, "output")?.expect("required output");
    Ok(known(paths(&matches)?, vec![output]))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Dwp {
    #[serde(flatten)]
    pub data: ToolData,
}

impl Job for Dwp {
    const TYPE: &'static str = "dwp";

    fn patterns() -> Vec<String> {
        vec![crate::registry::binary_pattern(r"dwp")]
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
    fn dwp_requires_explicit_inputs_and_output() {
        let effect = infer("dwp -o package.dwp one.dwo two.dwo").unwrap();
        assert_eq!(effect.inputs, paths_of(&["one.dwo", "two.dwo"]));
        assert_eq!(effect.outputs, paths_of(&["package.dwp"]));
        assert!(infer("dwp -e executable -o package.dwp").is_err());
        assert!(infer("dwp one.dwo").is_err());
    }
}
