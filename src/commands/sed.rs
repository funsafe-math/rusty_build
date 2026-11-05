use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use clap::Parser;
use serde::{Deserialize, Serialize};

use crate::job::{ExecutionContext, Job};
use crate::registry::command_pattern;
use crate::shell::ShellCommand;

use super::external::execute_original;

/// Sed's supported argument grammar, without guessing script filesystem effects.
#[derive(Clone, Debug, Parser, Serialize, Deserialize)]
#[command(disable_help_flag = true, disable_version_flag = true)]
pub struct Sed {
    #[arg(short = 'n', long = "quiet")]
    pub quiet: bool,
    #[arg(short = 'e', long = "expression")]
    pub expressions: Vec<String>,
    #[arg(short = 'f', long = "file")]
    pub script_files: Vec<PathBuf>,
    #[arg(long = "in-place", num_args = 0..=1, require_equals = true, default_missing_value = "")]
    pub in_place: Option<String>,
    pub operands: Vec<String>,
}

impl Job for Sed {
    const TYPE: &'static str = "sed";

    fn patterns() -> Vec<String> {
        vec![command_pattern("sed")]
    }

    fn parse(shell: &ShellCommand) -> Result<Self> {
        let mut value = false;
        let mut positional_only = false;
        let normalized = shell.args.iter().map(|arg| {
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
            if arg == "-i" {
                "--in-place=".into()
            } else if let Some(suffix) = arg.strip_prefix("-i") {
                format!("--in-place={suffix}")
            } else {
                arg.clone()
            }
        });
        let job = Self::try_parse_from(std::iter::once(shell.program.clone()).chain(normalized))?;
        if job.expressions.is_empty() && job.script_files.is_empty() && job.operands.is_empty() {
            bail!("sed requires an expression or script file");
        }
        Ok(job)
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

    // Default batching preserves a separate original-shell invocation per script.
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sed(text: &str) -> Sed {
        let shell = crate::shell::parse(text).unwrap().unwrap();
        <Sed as Job>::parse(&shell).unwrap()
    }

    #[test]
    fn in_place_suffix_and_script_values_are_not_confused() {
        assert_eq!(sed("sed -i 's/a/b/' file").in_place.as_deref(), Some(""));
        assert_eq!(
            sed("sed -i.bak 's/a/b/' file").in_place.as_deref(),
            Some(".bak")
        );
        let job = sed("sed -e 's/-input/x/' -f script.sed file");
        assert_eq!(job.expressions, ["s/-input/x/"]);
        assert_eq!(job.script_files, [PathBuf::from("script.sed")]);
    }

    #[test]
    fn positional_separator_preserves_literal_in_place_operand() {
        let job = sed("sed -- 's/a/b/' -input");
        assert_eq!(job.in_place, None);
        assert_eq!(job.operands, ["s/a/b/", "-input"]);
        assert!(job.unknown_effects());
        assert!(!job.cacheable());
    }
}
