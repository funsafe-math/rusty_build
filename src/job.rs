use std::{any::Any, fmt::Debug, fs};

use anyhow::{Context, Result};
use rayon::prelude::*;

use crate::shell::ShellHints;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
    ReadWrite,
}

impl Access {
    pub fn reads(self) -> bool {
        matches!(self, Self::Read | Self::ReadWrite)
    }
    pub fn writes(self) -> bool {
        matches!(self, Self::Write | Self::ReadWrite)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileAccess {
    pub path: String,
    pub access: Access,
}

impl FileAccess {
    pub fn read(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            access: Access::Read,
        }
    }
    pub fn write(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            access: Access::Write,
        }
    }
    pub fn read_write(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            access: Access::ReadWrite,
        }
    }
}

/// A read deliberately supplied by this producing job, even if it occurs later.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Producer {
    pub input: String,
    pub job: usize,
}

#[derive(Debug, Default)]
pub struct JobData {
    pub accesses: Vec<FileAccess>,
    pub producers: Vec<Producer>,
    pub unknown_effects: bool,
    pub shell: ShellHints,
    pub line: usize,
    pub text: String,
}

impl JobData {
    pub fn new(accesses: Vec<FileAccess>) -> Self {
        Self {
            accesses,
            ..Self::default()
        }
    }

    pub fn set_shell(&mut self, shell: ShellHints) {
        for path in shell.stdout.iter().chain(&shell.stderr) {
            if !is_null_device(path)
                && !self
                    .accesses
                    .iter()
                    .any(|a| a.path == *path && a.access.writes())
            {
                self.accesses.push(FileAccess::write(path));
            }
        }
        self.shell = shell;
    }
}

/// Tools own their operation and file effects. No execution strategy or stored
/// representation is needed: the DAG keeps these same concrete objects.
pub trait Job: Any + Debug {
    fn data(&self) -> &JobData;
    fn data_mut(&mut self) -> &mut JobData;
    fn name(&self) -> &str {
        std::any::type_name::<Self>()
    }
    fn execute(&self) -> Result<()>;

    fn execute_batch(&self, items: &[&dyn Job]) -> Result<()> {
        items.iter().try_for_each(|item| item.execute())
    }

    fn check_inputs(&self, inputs: &[&str]) -> Result<()> {
        if self.data().shell.ignore_failure {
            return Ok(());
        }
        check_inputs(inputs)
    }
}

/// This flag affects existence checks only, never command execution.
pub const PARALLEL_INPUT_CHECKS: bool = false;

pub fn check_inputs(inputs: &[&str]) -> Result<()> {
    let check = |path: &&str| {
        fs::metadata(*path)
            .map(|_| ())
            .with_context(|| format!("input does not exist or cannot be inspected: {path}"))
    };
    if PARALLEL_INPUT_CHECKS {
        inputs.par_iter().try_for_each(check)
    } else {
        inputs.iter().try_for_each(check)
    }
}

pub fn is_null_device(path: &str) -> bool {
    path == "/dev/null"
        || (cfg!(windows)
            && (path.eq_ignore_ascii_case("NUL") || path.eq_ignore_ascii_case("NUL:")))
}
