use anyhow::{Result, bail};
use clap::Parser;

use super::external::execute_argv;
use crate::{
    job::{FileAccess, Job, JobData},
    shell::ShellHints,
};

#[derive(Debug, Parser)]
#[command(disable_help_flag = true, disable_version_flag = true)]
struct SedArgs {
    #[arg(short = 'n', long = "quiet", alias = "silent")]
    quiet: bool,
    #[arg(short = 'E', short_alias = 'r', long = "regexp-extended")]
    extended: bool,
    #[arg(short = 'e', long = "expression")]
    expressions: Vec<String>,
    #[arg(short = 'f', long = "file")]
    script_files: Vec<String>,
    #[arg(long = "in-place", num_args = 0..=1, require_equals = true, default_missing_value = "")]
    in_place: Option<String>,
    operands: Vec<String>,
}

/// Original sed argv and the confidently understood subset of its file effects.
#[derive(Debug)]
pub struct Sed {
    pub data: JobData,
    pub argv: Vec<String>,
}

impl Sed {
    pub fn parse(argv: &[String], hints: &ShellHints) -> Result<Self> {
        // GNU -iSUFFIX has an optional attached value, never a following value.
        // Normalize only the metadata parser, not the command that is executed.
        let mut value = false;
        let mut positional_only = false;
        let normalized = argv.iter().skip(1).map(|arg| {
            if value {
                value = false;
                return arg.clone();
            }
            if positional_only {
                return arg.clone();
            }
            if arg == "--" {
                positional_only = true;
                return arg.clone();
            }
            if matches!(arg.as_str(), "-e" | "--expression" | "-f" | "--file") {
                value = true;
                return arg.clone();
            }
            if let Some(suffix) = arg.strip_prefix("-i") {
                format!("--in-place={suffix}")
            } else {
                arg.clone()
            }
        });
        let mut data = JobData {
            unknown_effects: true,
            ..JobData::default()
        };
        if let Some(program) = argv.first() {
            if let Ok(mut args) =
                SedArgs::try_parse_from(std::iter::once(program.clone()).chain(normalized))
            {
                if args.expressions.is_empty()
                    && args.script_files.is_empty()
                    && !args.operands.is_empty()
                {
                    args.expressions.push(args.operands.remove(0));
                }
                data.unknown_effects = !args.script_files.is_empty()
                    || args.expressions.is_empty()
                    || !args
                        .expressions
                        .iter()
                        .all(|script| simple_substitution(script));
                data.accesses.extend(
                    args.script_files
                        .into_iter()
                        .filter(|path| path != "-")
                        .map(FileAccess::read),
                );
                if args.operands.is_empty() {
                    data.unknown_effects = true;
                }
                for path in args.operands {
                    if path == "-" {
                        data.unknown_effects = true;
                        continue;
                    }
                    if let Some(suffix) = &args.in_place {
                        data.accesses.push(FileAccess::read_write(&path));
                        if !suffix.is_empty() {
                            let backup = if suffix.contains('*') {
                                suffix.replace('*', &path)
                            } else {
                                format!("{path}{suffix}")
                            };
                            data.accesses.push(FileAccess::write(backup));
                        }
                    } else {
                        data.accesses.push(FileAccess::read(path));
                    }
                }
            }
        }
        if hints.stdout.as_ref().is_some_and(|output| {
            data.accesses
                .iter()
                .any(|access| access.access.reads() && access.path == *output)
        }) {
            bail!(
                "sed input and stdout redirect must be different paths; use -i without that redirect"
            );
        }
        Ok(Self {
            data,
            argv: argv.to_vec(),
        })
    }
}

// A single s command cannot read or write additional files unless its flags
// contain r/w/e-like extensions. Reject everything beyond the understood grammar.
fn simple_substitution(script: &str) -> bool {
    if script.contains(['\n', '\r']) {
        return false;
    }
    let mut chars = script.chars();
    if chars.next() != Some('s') {
        return false;
    }
    let Some(delimiter) = chars.next() else {
        return false;
    };
    if delimiter == '\\' {
        return false;
    }
    for _ in 0..2 {
        let mut escaped = false;
        let mut closed = false;
        for ch in chars.by_ref() {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == delimiter {
                closed = true;
                break;
            }
        }
        if !closed {
            return false;
        }
    }
    chars.all(|ch| matches!(ch, 'g' | 'p' | 'i' | 'I' | 'm' | 'M' | '0'..='9'))
}

impl Job for Sed {
    fn data(&self) -> &JobData {
        &self.data
    }
    fn data_mut(&mut self) -> &mut JobData {
        &mut self.data
    }
    fn execute(&self) -> Result<()> {
        execute_argv(&self.argv, &self.data.shell)
    }
}
