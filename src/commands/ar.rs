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

use super::binary_common::{Effects, command, files, known, parse, paths};
use clap::Arg;

fn effects(shell: &ShellCommand) -> std::result::Result<Effects, String> {
    // Traditional ar's first word is an operation plus modifiers, optionally
    // prefixed by one dash. Only that positional word admits a leading dash.
    let matches = parse(
        command("ar")
            .arg(
                Arg::new("operation")
                    .required(true)
                    .allow_hyphen_values(true),
            )
            .arg(files(2, None)),
        shell,
    )?;
    let operation = matches
        .get_one::<String>("operation")
        .expect("required operation");
    let operation = operation.strip_prefix('-').unwrap_or(operation);
    let mut replace = false;
    for modifier in operation.chars() {
        match modifier {
            'r' if !replace => replace = true,
            'c' | 's' | 'D' => (),
            _ => return Err("only ar r with c/s/D modifiers has understood file effects; thin, extract, plugin, script and other modes remain opaque".into()),
        }
    }
    if !replace {
        return Err("only archive replacement (ar r) is understood".into());
    }
    let inputs = paths(&matches)?;
    // The existing archive is read as well as rewritten. Member files supply
    // replacements, and insertion creates no files outside the archive itself.
    let outputs = vec![inputs[0].clone()];
    Ok(known(inputs, outputs))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Ar {
    #[serde(flatten)]
    pub data: ToolData,
}

impl Job for Ar {
    const TYPE: &'static str = "ar";

    fn patterns() -> Vec<String> {
        vec![crate::registry::binary_pattern(r"ar")]
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
    fn archives_are_read_modify_write() {
        for text in ["ar rcs lib.a one.o two.o", "ar -rcs lib.a one.o two.o"] {
            let effect = infer(text).unwrap();
            assert_eq!(effect.inputs, paths_of(&["lib.a", "one.o", "two.o"]));
            assert_eq!(effect.outputs, paths_of(&["lib.a"]));
        }
    }

    #[test]
    fn opaque_arguments_request_fallback_without_losing_argv() {
        assert!(infer("ar rcT lib.a input.o").is_err());
        let shell = crate::shell::parse("ar rcT lib.a input.o")
            .unwrap()
            .unwrap();
        let job = Ar::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
        assert!(infer("ar x lib.a member.o").is_err());
        let shell = crate::shell::parse("ar x lib.a member.o").unwrap().unwrap();
        let job = Ar::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
        assert!(infer("ar -M script").is_err());
        let shell = crate::shell::parse("ar -M script").unwrap().unwrap();
        let job = Ar::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
        assert!(infer("ar rcs --plugin=plugin.so lib.a input.o").is_err());
        let shell = crate::shell::parse("ar rcs --plugin=plugin.so lib.a input.o")
            .unwrap()
            .unwrap();
        let job = Ar::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
    }
}
