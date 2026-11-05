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
    Effects, command, copy_options, files, flag, known, parse, path, paths, value,
};

fn effects(shell: &ShellCommand) -> std::result::Result<Effects, String> {
    // objcopy uses -S for strip-all, unlike strip's -s. Keep the
    // shared option's long spelling, assigning that short spelling here.
    let mut parser = copy_options(command("objcopy"))
        .mut_arg("strip-all", |arg| arg.short('S'))
        .arg(flag("strip-debug", Some('g')))
        .arg(value("binary-architecture", Some('B')))
        .arg(value("only-section", Some('j')))
        .arg(value("add-section", None))
        .arg(value("update-section", None))
        .arg(value("dump-section", None))
        .arg(files(1, Some(2)));
    for name in [
        "set-section-flags",
        "set-section-alignment",
        "rename-section",
    ] {
        parser = parser.arg(value(name, None));
    }
    let matches = parse(parser, shell)?;
    let mut operands = paths(&matches)?;
    let output = if operands.len() == 2 {
        operands.pop().expect("two object operands")
    } else {
        operands[0].clone()
    };
    let mut result = known(operands, vec![output]);
    for option in ["add-section", "update-section", "dump-section"] {
        for specification in matches.get_many::<String>(option).into_iter().flatten() {
            let (section, filename) = specification
                .split_once('=')
                .ok_or_else(|| format!("--{option} requires SECTION=FILE"))?;
            if section.is_empty() {
                return Err(format!("--{option} requires a named section"));
            }
            let filename = path(filename)?;
            if option == "dump-section" {
                result.outputs.push(filename);
            } else {
                result.inputs.push(filename);
            }
        }
    }
    Ok(result)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Objcopy {
    #[serde(flatten)]
    pub data: ToolData,
}

impl Job for Objcopy {
    const TYPE: &'static str = "objcopy";

    fn patterns() -> Vec<String> {
        vec![crate::registry::binary_pattern(r"objcopy")]
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
    fn objcopy_explicit_and_inplace_outputs() {
        let effect = infer("objcopy --only-section=.text input.o output.o").unwrap();
        assert_eq!(effect.inputs, paths_of(&["input.o"]));
        assert_eq!(effect.outputs, paths_of(&["output.o"]));
        let effect = infer("objcopy --only-section=.text input.o").unwrap();
        assert_eq!(effect.inputs, paths_of(&["input.o"]));
        assert_eq!(effect.outputs, paths_of(&["input.o"]));
    }

    #[test]
    fn objcopy_exposes_all_section_file_effects() {
        let effect = infer("objcopy --add-section=.one=one.bin --update-section .two=two.bin --dump-section=.text=text.bin --dump-section .data=data.bin input.o output.o").unwrap();
        assert_eq!(effect.inputs, paths_of(&["input.o", "one.bin", "two.bin"]));
        assert_eq!(
            effect.outputs,
            paths_of(&["output.o", "text.bin", "data.bin"])
        );
        let effect = infer("objcopy --dump-section=.text=dump.bin input.o").unwrap();
        assert_eq!(effect.outputs, paths_of(&["input.o", "dump.bin"]));
    }

    #[test]
    fn opaque_arguments_request_fallback_without_losing_argv() {
        assert!(infer("objcopy --unknown input.o output.o").is_err());
        let shell = crate::shell::parse("objcopy --unknown input.o output.o")
            .unwrap()
            .unwrap();
        let job = Objcopy::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
        assert!(infer("objcopy --add-gnu-debuglink=debug.o input.o").is_err());
        let shell = crate::shell::parse("objcopy --add-gnu-debuglink=debug.o input.o")
            .unwrap()
            .unwrap();
        let job = Objcopy::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
        assert!(infer("objcopy --add-section=.text input.o output.o").is_err());
        let shell = crate::shell::parse("objcopy --add-section=.text input.o output.o")
            .unwrap()
            .unwrap();
        let job = Objcopy::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
        assert!(infer("objcopy @options input.o output.o").is_err());
        let shell = crate::shell::parse("objcopy @options input.o output.o")
            .unwrap()
            .unwrap();
        let job = Objcopy::parse(&shell).unwrap();
        assert!(job.data.unknown_effects);
        assert_eq!(&job.data.argv[1..], shell.args);
    }
}
