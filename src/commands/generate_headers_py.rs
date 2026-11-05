use std::path::{Path, PathBuf};

use anyhow::{Result, ensure};
use clap::Parser;
use serde::{Deserialize, Serialize};

use crate::job::{BatchContext, BatchItem, BatchReport, ExecutionContext, Job};
use crate::registry::command_pattern;
use crate::shell::ShellCommand;

use super::external::execute_original;
use super::python::is_python;

const SCRIPT: &str = "tools/generate_header.py";

/// Opt-in effect contract for the repository's known Python header generator.
/// Matching this filename alone is not evidence that an arbitrary script is safe;
/// callers explicitly register this type only for workspaces trusting the script.
#[derive(Clone, Debug, Parser, Serialize, Deserialize)]
#[command(disable_help_flag = true, disable_version_flag = true)]
pub struct GenerateHeadersPy {
    #[arg(skip)]
    pub interpreter: String,
    pub script: PathBuf,
    pub template: PathBuf,
    #[arg(long)]
    pub output: PathBuf,
}

impl Job for GenerateHeadersPy {
    const TYPE: &'static str = "generate-header";

    fn patterns() -> Vec<String> {
        let interpreter = command_pattern(r"(?:python(?:[0-9]+(?:\.[0-9]+)*)?|py)");
        let prefix = interpreter
            .strip_suffix(r"(?:\s|$|[<>;&|])")
            .expect("command pattern suffix");
        vec![format!(
            r#"{prefix}\s+(?:tools/generate_header\.py|'tools/generate_header\.py'|"tools/generate_header\.py")(?:\s|$|[<>;&|])"#
        )]
    }

    fn parse(shell: &ShellCommand) -> Result<Self> {
        ensure!(
            is_python(&shell.program),
            "header generator requires a Python interpreter"
        );
        let mut job = Self::try_parse_from(
            std::iter::once(shell.program.as_str()).chain(shell.args.iter().map(String::as_str)),
        )?;
        ensure!(
            job.script == Path::new(SCRIPT),
            "the generator parser only models {SCRIPT}"
        );
        job.interpreter = shell.program.clone();
        Ok(job)
    }

    fn inputs(&self) -> impl Iterator<Item = &Path> {
        [self.script.as_path(), self.template.as_path()].into_iter()
    }

    fn outputs(&self) -> impl Iterator<Item = &Path> {
        std::iter::once(self.output.as_path())
    }

    fn execute(&self, ctx: &ExecutionContext<'_>) -> Result<()> {
        execute_original(ctx)
    }

    fn execute_batch(items: &[BatchItem<'_, Self>], ctx: &BatchContext<'_>) -> BatchReport {
        // Each script receives its original interpreter, argv and redirects.
        // Sharing an interpreter would change Python's process-global semantics.
        ctx.run_each(items, |job, execution| job.execute(execution))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::SourceCommand;
    use crate::job::ExecutionStrategy;
    use crate::registry::Registry;

    #[test]
    fn registration_declares_only_known_generator_effects() {
        let mut registry = Registry::default();
        registry.register::<GenerateHeadersPy>().unwrap();
        for text in [
            "python3 tools/generate_header.py templates/input.h.in --output generated/output.h",
            "/usr/bin/python3.12 'tools/generate_header.py' templates/input.h.in --output generated/output.h",
        ] {
            let record = registry
                .classify(SourceCommand {
                    line: 1,
                    text: text.into(),
                    target: None,
                })
                .unwrap();
            assert!(record.typed::<GenerateHeadersPy>().is_some());
            assert_eq!(
                record.spec.inputs,
                [PathBuf::from(SCRIPT), PathBuf::from("templates/input.h.in")]
            );
            assert_eq!(record.spec.outputs, [PathBuf::from("generated/output.h")]);
            assert!(!record.spec.unknown_effects);
            assert!(!record.spec.cacheable);
            assert_eq!(record.strategy, ExecutionStrategy::External);
        }
    }

    #[test]
    fn precise_effects_are_opt_in_and_options_are_validated() {
        let source = |text: &str| SourceCommand {
            line: 1,
            text: text.into(),
            target: None,
        };
        let record = Registry::default()
            .classify(source(
                "python3 tools/generate_header.py input --output result",
            ))
            .unwrap();
        assert!(record.spec.unknown_effects);
        let mut registry = Registry::default();
        registry.register::<GenerateHeadersPy>().unwrap();
        assert!(
            registry
                .classify(source("python3 tools/generate_header.py input"))
                .is_err()
        );
        assert!(
            registry
                .classify(source(
                    "python3 tools/generate_header.py input --output result --unknown"
                ))
                .is_err()
        );
    }

    #[test]
    fn typed_batch_executes_every_generator_and_reports_its_node() {
        use std::fs;
        use std::process::Command;
        use std::sync::atomic::AtomicBool;

        if Command::new("python3").arg("--version").output().is_err() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("tools")).unwrap();
        fs::write(
            root.path().join(SCRIPT),
            include_str!("../../fixtures/python_headers/tools/generate_header.py"),
        )
        .unwrap();
        fs::write(root.path().join("input"), "#define ANSWER @ANSWER@\n").unwrap();
        let mut registry = Registry::default();
        registry.register::<GenerateHeadersPy>().unwrap();
        let records = ["first.h", "second.h"].map(|output| {
            registry
                .classify(SourceCommand {
                    line: 1,
                    text: format!("python3 {SCRIPT} input --output {output}"),
                    target: None,
                })
                .unwrap()
        });
        let items: Vec<_> = records
            .iter()
            .enumerate()
            .map(|(id, record)| BatchItem {
                id,
                job: record.typed::<GenerateHeadersPy>().unwrap(),
                record,
            })
            .collect();
        let cancelled = AtomicBool::new(false);
        let report = GenerateHeadersPy::execute_batch(
            &items,
            &BatchContext {
                cwd: root.path(),
                cancelled: &cancelled,
            },
        );
        assert_eq!(report.outcomes.len(), 2);
        for (expected_id, (id, result)) in report.outcomes.into_iter().enumerate() {
            assert_eq!(id, expected_id);
            result.unwrap();
        }
        for output in ["first.h", "second.h"] {
            assert_eq!(
                fs::read_to_string(root.path().join(output)).unwrap(),
                "#define ANSWER 42\n"
            );
        }
    }
}
