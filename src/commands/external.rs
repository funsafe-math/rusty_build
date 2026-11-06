use crate::{
    job::{Job, JobData, is_null_device},
    shell::{ShellHints, Stream},
};
use anyhow::{Context, Result, bail, ensure};
use std::{
    fs::File,
    io::{self, Write},
    process::{Command, ExitStatus, Stdio},
};

/// Explicitly registered commands with effects that are not understood.
#[derive(Debug)]
pub struct External {
    pub argv: Vec<String>,
    pub data: JobData,
}

impl External {
    pub fn parse(argv: &[String], _shell: &ShellHints) -> Result<Self> {
        ensure!(!argv.is_empty(), "missing executable");
        Ok(Self {
            argv: argv.to_vec(),
            data: JobData {
                unknown_effects: true,
                ..Default::default()
            },
        })
    }
}

impl Job for External {
    fn data(&self) -> &JobData {
        &self.data
    }
    fn data_mut(&mut self) -> &mut JobData {
        &mut self.data
    }
    fn name(&self) -> &str {
        &self.argv[0]
    }
    fn execute(&self) -> Result<()> {
        execute_argv(&self.argv, &self.data.shell)
    }
}

pub fn finish(shell: &ShellHints, result: Result<()>) -> Result<()> {
    if shell.ignore_failure { Ok(()) } else { result }
}

pub fn configure_stdio(command: &mut Command, shell: &ShellHints) -> Result<()> {
    for (stream, path) in shell.redirects() {
        let target = if is_null_device(path) {
            Stdio::null()
        } else {
            Stdio::from(File::create(path).with_context(|| format!("open output {path}"))?)
        };
        match stream {
            Stream::Stdout => {
                command.stdout(target);
            }
            Stream::Stderr => {
                command.stderr(target);
            }
        }
    }
    Ok(())
}

pub fn command_status(argv: &[String], shell: &ShellHints) -> Result<ExitStatus> {
    let (program, args) = argv.split_first().context("missing executable")?;
    let mut command = Command::new(program);
    command.args(args);
    configure_stdio(&mut command, shell)?;
    command.status().with_context(|| format!("start {program}"))
}

pub fn execute_argv(argv: &[String], shell: &ShellHints) -> Result<()> {
    finish(
        shell,
        (|| {
            let status = command_status(argv, shell)?;
            if !status.success() {
                bail!("{} failed ({status})", argv[0]);
            }
            Ok(())
        })(),
    )
}

/// Native filesystem utilities have no command stdout. Still open/truncate
/// redirects before the operation, just as the original command would.
pub fn execute_native(shell: &ShellHints, operation: impl FnOnce() -> Result<()>) -> Result<()> {
    finish(
        shell,
        (|| {
            let mut stdout = None;
            let mut stderr = None;
            for (stream, path) in shell.redirects() {
                let file = if is_null_device(path) {
                    None
                } else {
                    Some(File::create(path).with_context(|| format!("open output {path}"))?)
                };
                match stream {
                    Stream::Stdout => stdout = file,
                    Stream::Stderr => stderr = file,
                }
            }
            let result = operation();
            if let Err(error) = result {
                if let Some(file) = &mut stderr {
                    writeln!(file, "{error:#}")?;
                } else if shell
                    .stderr
                    .as_deref()
                    .is_none_or(|path| !is_null_device(path))
                {
                    writeln!(io::stderr(), "{error:#}")?;
                }
                bail!("native command failed");
            }
            drop(stdout);
            Ok(())
        })(),
    )
}
