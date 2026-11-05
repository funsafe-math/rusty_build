use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::job::{ExecutionContext, Job, is_null_device, requires_original_shell};
use crate::shell::ShellCommand;

/// An unmodeled command: its identity is known, but its filesystem effects are not.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct External {
    pub kind: String,
    pub argv: Vec<String>,
    pub argument_parsing: String,
}

impl External {
    pub fn opaque(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            argv: Vec::new(),
            argument_parsing: "opaque".into(),
        }
    }

    pub fn from_shell(kind: impl Into<String>, shell: &ShellCommand) -> Self {
        Self {
            kind: kind.into(),
            argv: std::iter::once(shell.program.clone())
                .chain(shell.args.iter().cloned())
                .collect(),
            argument_parsing: "passthrough".into(),
        }
    }
}

impl Job for External {
    const TYPE: &'static str = "external";

    fn parse(shell: &ShellCommand) -> Result<Self> {
        Ok(Self::from_shell(Self::TYPE, shell))
    }

    fn kind(&self) -> &str {
        &self.kind
    }

    fn inputs(&self) -> impl Iterator<Item = &Path> {
        std::iter::empty()
    }

    fn outputs(&self) -> impl Iterator<Item = &Path> {
        std::iter::empty()
    }

    fn unknown_effects(&self) -> bool {
        true
    }

    fn execute(&self, ctx: &ExecutionContext<'_>) -> Result<()> {
        execute_original(ctx)
    }

    // The default batch implementation executes each opaque shell independently.
}

/// Preserve all Bash syntax and descriptor-opening semantics of the source log.
pub fn execute_original(ctx: &ExecutionContext<'_>) -> Result<()> {
    let status = Command::new("bash")
        .args(["--noprofile", "--norc", "-c"])
        .arg(&ctx.record.source.text)
        .env_remove("BASH_ENV")
        .env_remove("ENV")
        .current_dir(ctx.cwd)
        .status()
        .context("starting bash for original command")?;
    if !status.success() {
        bail!(
            "command on line {} failed ({status}): {}",
            ctx.record.source.line,
            ctx.record.source.text
        );
    }
    Ok(())
}

/// Execute typed arguments, unless original shell semantics must be retained.
pub fn execute_argv(argv: &[String], ctx: &ExecutionContext<'_>) -> Result<()> {
    let Some(shell) = ctx.record.shell.as_ref() else {
        return execute_original(ctx);
    };
    if requires_original_shell(Some(shell)) {
        return execute_original(ctx);
    }
    let Some((program, args)) = argv.split_first() else {
        bail!(
            "command on line {} has no executable",
            ctx.record.source.line
        );
    };
    let mut command = Command::new(program);
    command.args(args).current_dir(ctx.cwd);
    apply_null_streams(&mut command, Some(shell));
    let status = command.status().with_context(|| {
        format!(
            "starting {program} for command on line {}",
            ctx.record.source.line
        )
    })?;
    if !status.success() {
        bail!(
            "command on line {} failed ({status}): {}",
            ctx.record.source.line,
            ctx.record.source.text
        );
    }
    Ok(())
}

/// Honor portable null sinks without a Bash process or opening /dev/null as a
/// literal Windows path. Meaningful file redirects still use the source shell.
pub(crate) fn apply_null_streams(command: &mut Command, shell: Option<&ShellCommand>) {
    let Some(shell) = shell else { return };
    if shell
        .stdin
        .as_ref()
        .is_some_and(|path| is_null_device(path))
    {
        command.stdin(Stdio::null());
    }
    if shell
        .stdout
        .as_ref()
        .is_some_and(|path| is_null_device(path))
    {
        command.stdout(Stdio::null());
    }
    if shell
        .stderr
        .as_ref()
        .is_some_and(|path| is_null_device(path))
    {
        command.stderr(Stdio::null());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::SourceCommand;
    use crate::job::JobRecord;

    #[test]
    fn redirected_commands_execute_original_shell_instead_of_replacement_argv() {
        let root = tempfile::tempdir().unwrap();
        for text in [
            "printf 'redirected\\n' > result",
            "printf 'appended\\n' >> result",
        ] {
            let source = SourceCommand {
                line: 3,
                text: text.into(),
                target: None,
            };
            let shell = crate::shell::parse(text).unwrap().unwrap();
            let job = External::from_shell("test", &shell);
            let record = JobRecord::new(source, Some(shell), job).unwrap();
            execute_argv(
                &["false".into()],
                &ExecutionContext {
                    cwd: root.path(),
                    record: &record,
                },
            )
            .unwrap();
        }
        assert_eq!(
            std::fs::read(root.path().join("result")).unwrap(),
            b"redirected\nappended\n"
        );
    }

    #[test]
    fn opaque_shell_never_flattens_into_replacement_argv() {
        let root = tempfile::tempdir().unwrap();
        let source = SourceCommand {
            line: 4,
            text: "printf first > result && printf second >> result".into(),
            target: None,
        };
        let record = JobRecord::new(source, None, External::opaque("test")).unwrap();
        execute_argv(
            &["false".into()],
            &ExecutionContext {
                cwd: root.path(),
                record: &record,
            },
        )
        .unwrap();
        assert_eq!(
            std::fs::read(root.path().join("result")).unwrap(),
            b"firstsecond"
        );
    }
}
