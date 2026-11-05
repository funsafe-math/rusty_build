//! Shared argv preservation and conservative file-effect guards for binary tools.
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::{Result, bail};
use clap::{Arg, ArgAction, ArgMatches, Args, Command, Parser};
use serde::{Deserialize, Serialize};

use crate::{
    job::{JobRecord, JobSpec},
    shell::ShellCommand,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolData {
    pub argv: Vec<String>,
    pub inputs: Vec<PathBuf>,
    pub outputs: Vec<PathBuf>,
    pub unknown_effects: bool,
    pub argument_parsing: String,
    pub effect_reason: String,
}

#[derive(Debug)]
pub(crate) struct Effects {
    pub inputs: Vec<PathBuf>,
    pub outputs: Vec<PathBuf>,
    pub debug_lookup: bool,
}

impl ToolData {
    pub(crate) fn from_effects(shell: &ShellCommand, effects: Result<Effects, String>) -> Self {
        let mut data = Self {
            argv: std::iter::once(shell.program.clone())
                .chain(shell.args.iter().cloned())
                .collect(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            unknown_effects: true,
            argument_parsing: "passthrough".into(),
            effect_reason: String::new(),
        };
        match effects {
            Ok(effects) => {
                data.inputs = effects.inputs;
                data.outputs = effects.outputs;
                if effects.debug_lookup {
                    data.effect_reason =
                        "mode may discover separate debug files; complete inputs are not known"
                            .into();
                } else {
                    data.unknown_effects = false;
                    data.argument_parsing = "effects".into();
                    data.effect_reason =
                        "known file operands and supported option semantics".into();
                }
            }
            Err(reason) => data.effect_reason = reason,
        }
        data
    }
}

pub(crate) fn parse_args<T: Parser>(shell: &ShellCommand) -> Result<T, String> {
    // GNU expands response files even after `--`, before option parsing.
    reject_response_files(shell)?;
    T::try_parse_from(
        std::iter::once(shell.program.as_str()).chain(shell.args.iter().map(String::as_str)),
    )
    .map_err(|error| error.to_string())
}

fn reject_response_files(shell: &ShellCommand) -> Result<(), String> {
    if shell.args.iter().any(|arg| arg.starts_with('@')) {
        return Err("response files can introduce untracked arguments and inputs".into());
    }
    Ok(())
}

pub(crate) fn objects(mut files: Vec<PathBuf>, default: Option<&str>) -> Result<Effects, String> {
    if files.is_empty() {
        let Some(default) = default else {
            return Err("this inspection mode needs explicit object files".into());
        };
        files.push(default.into());
    }
    if files.iter().any(|file| file.as_os_str() == "-") {
        return Err("dash operands have tool-specific stream semantics".into());
    }
    Ok(known(files, Vec::new()))
}

pub(crate) fn require_stdin(shell: &ShellCommand) -> Result<(), String> {
    if shell.stdin.is_none() {
        return Err("this mode reads stdin without an explicit input redirection".into());
    }
    Ok(())
}

#[derive(Args)]
pub(crate) struct Demangle {
    // Optional values must use `=` so bare -C never consumes an object filename.
    #[arg(short = 'C', long = "demangle", num_args = 0..=1, require_equals = true, default_missing_value = "auto")]
    _style: Option<String>,
    #[arg(long = "no-demangle")]
    _no_demangle: bool,
    #[arg(long = "recurse-limit", alias = "recursion-limit")]
    _recurse_limit: bool,
    #[arg(long = "no-recurse-limit", alias = "no-recursion-limit")]
    _no_recurse_limit: bool,
}

pub(crate) fn command(name: &'static str) -> Command {
    Command::new(name)
        .disable_help_flag(true)
        .disable_version_flag(true)
}

pub(crate) fn flag(name: &'static str, short: Option<char>) -> Arg {
    let arg = Arg::new(name)
        .long(name)
        .action(ArgAction::SetTrue)
        .overrides_with(name);
    match short {
        Some(short) => arg.short(short),
        None => arg,
    }
}

pub(crate) fn short_flag(name: &'static str, short: char) -> Arg {
    Arg::new(name)
        .short(short)
        .action(ArgAction::SetTrue)
        .overrides_with(name)
}

pub(crate) fn value(name: &'static str, short: Option<char>) -> Arg {
    let arg = Arg::new(name).long(name).action(ArgAction::Append);
    match short {
        Some(short) => arg.short(short),
        None => arg,
    }
}

pub(crate) fn files(min: usize, max: Option<usize>) -> Arg {
    let arg = Arg::new("files").required(true);
    match max {
        Some(max) => arg.num_args(min..=max),
        None => arg.num_args(min..),
    }
}

pub(crate) fn parse(command: Command, shell: &ShellCommand) -> Result<ArgMatches, String> {
    reject_response_files(shell)?;
    command
        .try_get_matches_from(std::iter::once(&shell.program).chain(shell.args.iter()))
        .map_err(|error| error.to_string())
}

pub(crate) fn paths(matches: &ArgMatches) -> Result<Vec<PathBuf>, String> {
    matches
        .get_many::<String>("files")
        .into_iter()
        .flatten()
        .map(|name| path(name))
        .collect()
}

pub(crate) fn path(name: &str) -> Result<PathBuf, String> {
    if name.is_empty() || name == "-" {
        return Err(
            "empty filenames or implicit standard streams have unknown file effects".into(),
        );
    }
    Ok(PathBuf::from(name))
}

pub(crate) fn last_path(matches: &ArgMatches, name: &str) -> Result<Option<PathBuf>, String> {
    matches
        .get_many::<String>(name)
        .and_then(|values| values.last())
        .map(|name| path(name))
        .transpose()
}

pub(crate) fn known(inputs: Vec<PathBuf>, outputs: Vec<PathBuf>) -> Effects {
    Effects {
        inputs,
        outputs,
        debug_lookup: false,
    }
}

pub(crate) fn inplace(files: Vec<PathBuf>) -> Effects {
    known(files.clone(), files)
}

// The common objcopy/strip option subset; each command adds its own grammar.
pub(crate) fn copy_options(mut command: Command) -> Command {
    for (name, short) in [
        ("input-target", Some('I')),
        ("output-target", Some('O')),
        ("target", Some('F')),
        ("remove-section", Some('R')),
        ("keep-section", None),
        ("keep-symbol", Some('K')),
        ("strip-symbol", Some('N')),
    ] {
        command = command.arg(value(name, short));
    }
    for (name, short) in [
        ("strip-all", Some('s')),
        ("strip-unneeded", None),
        ("discard-all", Some('x')),
        ("discard-locals", Some('X')),
        ("only-keep-debug", None),
        ("keep-file-symbols", None),
        ("preserve-dates", Some('p')),
        ("enable-deterministic-archives", Some('D')),
        ("disable-deterministic-archives", Some('U')),
        ("verbose", Some('v')),
        ("wildcard", Some('w')),
    ] {
        command = command.arg(flag(name, short));
    }
    command
}

fn is_precise(spec: &JobSpec) -> bool {
    spec.parsed
        .get("argument_parsing")
        .and_then(|value| value.as_str())
        == Some("effects")
        && !spec.unknown_effects
}

/// Existing aliases/thin archives fall back during planning. Newly introduced
/// ones are refused at runtime rather than executed against stale hazards.
fn uncertainty(spec: &JobSpec, cwd: &Path, check_archives: bool) -> Result<Option<String>> {
    for path in spec.inputs.iter().chain(&spec.outputs) {
        let absolute = if path.is_absolute() {
            path.clone()
        } else {
            cwd.join(path)
        };
        for ancestor in absolute.ancestors() {
            if fs::symlink_metadata(ancestor).is_ok_and(|meta| meta.file_type().is_symlink()) {
                return Ok(Some(format!(
                    "symlink alias in file effects: {}",
                    ancestor.display()
                )));
            }
        }
        let meta = match fs::metadata(&absolute) {
            Ok(meta) => meta,
            Err(_) => continue, // The original tool reports missing/access failures.
        };
        if !meta.is_file() {
            return Ok(Some(format!("non-regular operand: {}", path.display())));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if meta.nlink() > 1 {
                return Ok(Some(format!(
                    "hardlink alias in file effects: {}",
                    path.display()
                )));
            }
        }
    }
    if check_archives {
        for path in &spec.inputs {
            let absolute = if path.is_absolute() {
                path.clone()
            } else {
                cwd.join(path)
            };
            if let Ok(mut file) = fs::File::open(&absolute) {
                let mut magic = [0u8; 8];
                match file.read_exact(&mut magic) {
                    Ok(()) if &magic == b"!<thin>\n" => {
                        return Ok(Some(format!(
                            "thin archive has external member inputs: {}",
                            path.display()
                        )));
                    }
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {}
                    Err(error) => {
                        return Ok(Some(format!(
                            "cannot validate input format {}: {error}",
                            path.display()
                        )));
                    }
                }
            }
        }
    }
    Ok(None)
}

pub(crate) fn refine_effects(spec: &mut JobSpec, cwd: &Path, check_archives: bool) -> Result<()> {
    if is_precise(spec) {
        if let Some(reason) = uncertainty(spec, cwd, check_archives)? {
            spec.unknown_effects = true;
            spec.effect_reason = Some(reason);
        }
    }
    Ok(())
}

pub(crate) fn validate(record: &JobRecord, cwd: &Path, check_archives: bool) -> Result<()> {
    if is_precise(&record.spec) {
        if let Some(reason) = uncertainty(&record.spec, cwd, check_archives)? {
            bail!(
                "precise tool effects no longer valid ({reason}); re-plan now that the operand exists"
            );
        }
    }
    Ok(())
}
