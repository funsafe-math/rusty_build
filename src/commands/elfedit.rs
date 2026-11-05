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

use super::binary_common::{Effects, command, files, inplace, parse, paths, value};

fn effects(shell: &ShellCommand) -> std::result::Result<Effects, String> {
    let mut parser = command("elfedit").arg(files(1, None));
    for name in [
        "input-mach",
        "output-mach",
        "input-type",
        "output-type",
        "input-osabi",
        "output-osabi",
        "input-abiversion",
        "output-abiversion",
    ] {
        parser = parser.arg(value(name, None));
    }
    let matches = parse(parser, shell)?;
    Ok(inplace(paths(&matches)?))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Elfedit {
    #[serde(flatten)]
    pub data: ToolData,
}

impl Job for Elfedit {
    const TYPE: &'static str = "elfedit";

    fn patterns() -> Vec<String> {
        vec![crate::registry::binary_pattern(r"elfedit")]
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
    fn elf_header_edit_is_inplace() {
        let effect = infer("elfedit --output-osabi GNU input.o").unwrap();
        assert_eq!(effect.inputs, paths_of(&["input.o"]));
        assert_eq!(effect.outputs, effect.inputs);
    }

    #[test]
    fn opaque_arguments_request_fallback_without_losing_argv() {
        assert!(infer("elfedit --unknown input.o").is_err());
        let shell = crate::shell::parse("elfedit --unknown input.o")
            .unwrap()
            .unwrap();
        let job = Elfedit::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
    }
}
