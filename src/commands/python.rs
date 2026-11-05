use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::job::{ExecutionContext, Job};
use crate::shell::ShellCommand;

use super::external::{apply_null_streams, execute_original};
use crate::job::requires_original_shell;

/// A configured script identity, deliberately independent of interpreter spelling.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Python {
    pub identity: String,
    pub interpreter: String,
    pub script: String,
    pub args: Vec<String>,
}

impl Python {
    pub fn identified(identity: &str, script: &str, shell: &ShellCommand) -> Result<Self> {
        ensure!(
            !identity.is_empty() && !script.is_empty(),
            "Python configuration requires an identity and script"
        );
        ensure!(
            is_python(&shell.program) && shell.args.first().is_some_and(|arg| arg == script),
            "expected Python interpreter and exact script {script}"
        );
        Ok(Self {
            identity: identity.into(),
            interpreter: shell.program.clone(),
            script: script.into(),
            args: shell.args[1..].to_vec(),
        })
    }
}

impl Job for Python {
    const TYPE: &'static str = "python";

    fn parse(shell: &ShellCommand) -> Result<Self> {
        let script = shell
            .args
            .first()
            .ok_or_else(|| anyhow::anyhow!("Python requires a script"))?;
        Self::identified(Self::TYPE, script, shell)
    }

    fn kind(&self) -> &str {
        &self.identity
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
        if ctx.record.shell.is_none() || requires_original_shell(ctx.record.shell.as_ref()) {
            return execute_original(ctx);
        }
        let mut command = std::process::Command::new(&self.interpreter);
        command
            .arg(&self.script)
            .args(&self.args)
            .current_dir(ctx.cwd);
        apply_null_streams(&mut command, ctx.record.shell.as_ref());
        let status = command
            .status()
            .with_context(|| format!("starting Python interpreter {}", self.interpreter))?;
        if !status.success() {
            bail!(
                "Python command on line {} failed ({status}): {}",
                ctx.record.source.line,
                ctx.record.source.text
            );
        }
        Ok(())
    }

    // Default batching preserves an independent process for every opaque script.
}

pub fn is_python(program: &str) -> bool {
    let name = if cfg!(windows) {
        program.rsplit(['/', '\\']).next().unwrap_or(program)
    } else {
        program.rsplit('/').next().unwrap_or(program)
    };
    let name = name.strip_suffix(".exe").unwrap_or(name);
    if name == "py" || name == "python" {
        return true;
    }
    name.strip_prefix("python").is_some_and(|version| {
        !version.is_empty()
            && version
                .split('.')
                .all(|part| !part.is_empty() && part.bytes().all(|c| c.is_ascii_digit()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_separate_from_interpreter_and_effects_stay_opaque() {
        for text in [
            "python 'scripts/make data.py' --out result",
            "/usr/bin/python3.12 'scripts/make data.py'",
            "py 'scripts/make data.py'",
        ] {
            let shell = crate::shell::parse(text).unwrap().unwrap();
            let job = Python::identified("generate", "scripts/make data.py", &shell).unwrap();
            assert_eq!(job.kind(), "generate");
            assert_eq!(job.interpreter, shell.program);
            assert!(job.unknown_effects() && !job.cacheable());
            assert_eq!(job.inputs().count(), 0);
            assert_eq!(job.outputs().count(), 0);
        }
    }

    #[test]
    fn exact_script_and_interpreter_are_required() {
        for text in [
            "python different.py",
            "python -O build.py",
            "pythonfuture build.py",
            "not-python build.py",
        ] {
            let shell = crate::shell::parse(text).unwrap().unwrap();
            assert!(Python::identified("generate", "build.py", &shell).is_err());
        }
        for name in ["python", "py", "/usr/bin/python3", "python3.12"] {
            assert!(is_python(name));
        }
        for name in ["python.", "python3.", "python3..1", "pythonfuture"] {
            assert!(!is_python(name));
        }
    }
}
