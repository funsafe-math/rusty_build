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

use super::binary_common::Effects;

// Argv alone cannot close this tool's read set; keep every option opaque.
fn effects(_shell: &ShellCommand) -> std::result::Result<Effects, String> {
    Err(
        "link inputs may be scripts with implicit file or library dependencies, even with -r"
            .into(),
    )
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Linker {
    #[serde(flatten)]
    pub data: ToolData,
}

impl Job for Linker {
    const TYPE: &'static str = "ld";

    fn patterns() -> Vec<String> {
        vec![crate::registry::binary_pattern(
            r"(?:ld(?:\.(?:bfd|gold|lld))?|lld|gold)",
        )]
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

    #[test]
    fn opaque_arguments_request_fallback_without_losing_argv() {
        assert!(infer("ld -r -o combined.o one.o two.o").is_err());
        let shell = crate::shell::parse("ld -r -o combined.o one.o two.o")
            .unwrap()
            .unwrap();
        let job = Linker::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
        assert!(infer("ld -r -T layout.ld -o combined.o one.o").is_err());
        let shell = crate::shell::parse("ld -r -T layout.ld -o combined.o one.o")
            .unwrap()
            .unwrap();
        let job = Linker::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
    }
}
