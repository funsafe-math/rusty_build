use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::job::{ExecutionContext, Job};
use crate::registry::command_pattern;
use crate::shell::ShellCommand;

use super::external::execute_argv;

/// External awk execution with only unambiguous command-line inputs described.
/// Program text is never interpreted to infer its filesystem effects.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Gawk {
    pub argv: Vec<String>,
    pub programs: Vec<String>,
    pub script_files: Vec<PathBuf>,
    pub field_separators: Vec<String>,
    pub assignments: Vec<String>,
    pub operands: Vec<String>,
    pub data_files: Vec<PathBuf>,
    /// False when an unknown option makes the remaining operand grammar unknown.
    pub metadata_complete: bool,
}

impl Job for Gawk {
    const TYPE: &'static str = "gawk";

    fn patterns() -> Vec<String> {
        vec![command_pattern("gawk")]
    }

    fn parse(shell: &ShellCommand) -> Result<Self> {
        let mut job = Self {
            argv: std::iter::once(shell.program.clone())
                .chain(shell.args.iter().cloned())
                .collect(),
            programs: Vec::new(),
            script_files: Vec::new(),
            field_separators: Vec::new(),
            assignments: Vec::new(),
            operands: Vec::new(),
            data_files: Vec::new(),
            metadata_complete: true,
        };
        let mut index = 0;
        let mut options = true;
        let mut informational = false;
        while let Some(arg) = shell.args.get(index) {
            if options && arg == "--" {
                options = false;
                index += 1;
                continue;
            }
            if options {
                let value_option = if arg.starts_with("--") {
                    let (name, attached) = arg
                        .split_once('=')
                        .map_or((arg.as_str(), None), |(name, value)| (name, Some(value)));
                    match name {
                        "--file" | "--source" | "--field-separator" | "--assign" => {
                            Some((name, attached))
                        }
                        _ => None,
                    }
                } else if arg.starts_with('-') && arg.len() >= 2 {
                    match &arg.as_bytes()[..2] {
                        b"-f" => Some(("--file", arg.get(2..).filter(|v| !v.is_empty()))),
                        b"-e" => Some(("--source", arg.get(2..).filter(|v| !v.is_empty()))),
                        b"-F" => {
                            Some(("--field-separator", arg.get(2..).filter(|v| !v.is_empty())))
                        }
                        b"-v" => Some(("--assign", arg.get(2..).filter(|v| !v.is_empty()))),
                        _ => None,
                    }
                } else {
                    None
                };
                if let Some((option, attached)) = value_option {
                    let value = match attached {
                        Some(value) => value,
                        None => {
                            index += 1;
                            shell.args.get(index).map(String::as_str).ok_or_else(|| {
                                anyhow::anyhow!("gawk option {arg} requires a value")
                            })?
                        }
                    };
                    match option {
                        "--file" => {
                            if value.is_empty() {
                                bail!("gawk option {arg} requires a nonempty script filename");
                            }
                            job.script_files.push(value.into());
                        }
                        "--source" => job.programs.push(value.into()),
                        "--field-separator" => job.field_separators.push(value.into()),
                        "--assign" => job.assignments.push(value.into()),
                        _ => unreachable!(),
                    }
                    index += 1;
                    continue;
                }
                match arg.as_str() {
                    "-h" | "--help" | "-V" | "--version" | "-C" | "--copyright" => {
                        informational = true;
                        index += 1;
                        continue;
                    }
                    "-b"
                    | "--characters-as-bytes"
                    | "-c"
                    | "--traditional"
                    | "-M"
                    | "--bignum"
                    | "-n"
                    | "--non-decimal-data"
                    | "-N"
                    | "--use-lc-numeric"
                    | "-O"
                    | "--optimize"
                    | "-P"
                    | "--posix"
                    | "-r"
                    | "--re-interval"
                    | "-s"
                    | "--no-optimize"
                    | "-S"
                    | "--sandbox"
                    | "-t"
                    | "--lint-old" => {
                        index += 1;
                        continue;
                    }
                    _ if arg.starts_with('-') && arg != "-" => {
                        // Unknown options may consume any following token. Keep argv
                        // intact without calling their values programs or data files.
                        job.metadata_complete = false;
                        break;
                    }
                    _ => {}
                }
            }
            if job.programs.is_empty() && job.script_files.is_empty() {
                job.programs.push(arg.clone());
                index += 1;
            }
            // After the program (or the first data operand with -f/-e), awk
            // treats remaining arguments as operands, not command-line options.
            job.operands.extend(shell.args[index..].iter().cloned());
            job.data_files.extend(
                job.operands
                    .iter()
                    .filter(|arg| arg.as_str() != "-" && !is_assignment(arg))
                    .map(PathBuf::from),
            );
            break;
        }
        if job.metadata_complete
            && !informational
            && job.programs.is_empty()
            && job.script_files.is_empty()
        {
            bail!("gawk requires a program or script file");
        }
        Ok(job)
    }

    fn inputs(&self) -> impl Iterator<Item = &Path> {
        self.script_files
            .iter()
            .chain(&self.data_files)
            .filter(|path| path.as_path() != Path::new("-"))
            .map(PathBuf::as_path)
    }

    fn outputs(&self) -> impl Iterator<Item = &Path> {
        std::iter::empty()
    }

    fn unknown_effects(&self) -> bool {
        true
    }

    fn execute(&self, ctx: &ExecutionContext<'_>) -> Result<()> {
        execute_argv(&self.argv, ctx)
    }
}

fn is_assignment(operand: &str) -> bool {
    let Some((name, _)) = operand.split_once('=') else {
        return false;
    };
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}
