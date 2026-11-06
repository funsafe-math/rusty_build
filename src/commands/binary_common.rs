//! Conservative file effects for external binary tools; paths retain their spelling.
use crate::job::{FileAccess, JobData};
use clap::{Arg, ArgAction, ArgMatches, Args, Command, Parser};
use std::{fs::File, io::Read};

#[derive(Debug)]
pub(crate) struct Effects {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub debug_lookup: bool,
}

pub(crate) fn job_data(effects: Result<Effects, String>) -> JobData {
    let Ok(effects) = effects else {
        return JobData {
            unknown_effects: true,
            ..JobData::default()
        };
    };
    // A thin archive names external members that are absent from argv. Keep
    // the explicit accesses, but never claim they describe the complete read set.
    let thin_archive = effects.inputs.iter().any(|path| {
        let Ok(mut file) = File::open(path) else {
            return false;
        };
        let mut magic = [0; 8];
        file.read_exact(&mut magic).is_ok() && &magic == b"!<thin>\n"
    });
    let mut accesses = Vec::new();
    for path in effects.inputs {
        if !accesses
            .iter()
            .any(|access: &FileAccess| access.path == path)
        {
            accesses.push(FileAccess::read(path));
        }
    }
    for path in effects.outputs {
        match accesses.iter_mut().find(|access| access.path == path) {
            Some(access) => access.access = crate::job::Access::ReadWrite,
            None => accesses.push(FileAccess::write(path)),
        }
    }
    JobData {
        accesses,
        unknown_effects: effects.debug_lookup || thin_archive,
        ..JobData::default()
    }
}

pub(crate) fn parse_args<T: Parser>(argv: &[String]) -> Result<T, String> {
    reject_response_files(argv)?;
    T::try_parse_from(argv).map_err(|error| error.to_string())
}

fn reject_response_files(argv: &[String]) -> Result<(), String> {
    if argv.iter().skip(1).any(|arg| arg.starts_with('@')) {
        return Err("response files can introduce untracked arguments and inputs".into());
    }
    Ok(())
}

pub(crate) fn objects(mut files: Vec<String>, default: Option<&str>) -> Result<Effects, String> {
    if files.is_empty() {
        let Some(default) = default else {
            return Err("this inspection mode needs explicit object files".into());
        };
        files.push(default.into());
    }
    if files.iter().any(|file| file == "-") {
        return Err("dash operands have tool-specific stream semantics".into());
    }
    Ok(known(files, Vec::new()))
}

pub(crate) fn require_stdin(_argv: &[String]) -> Result<(), String> {
    Err("implicit stdin is not a known file input".into())
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

pub(crate) fn parse(command: Command, argv: &[String]) -> Result<ArgMatches, String> {
    reject_response_files(argv)?;
    command
        .try_get_matches_from(argv.iter())
        .map_err(|error| error.to_string())
}

pub(crate) fn paths(matches: &ArgMatches) -> Result<Vec<String>, String> {
    matches
        .get_many::<String>("files")
        .into_iter()
        .flatten()
        .map(|name| path(name))
        .collect()
}

pub(crate) fn path(name: &str) -> Result<String, String> {
    if name.is_empty() || name == "-" {
        return Err(
            "empty filenames or implicit standard streams have unknown file effects".into(),
        );
    }
    Ok(name.to_owned())
}

pub(crate) fn last_path(matches: &ArgMatches, name: &str) -> Result<Option<String>, String> {
    matches
        .get_many::<String>(name)
        .and_then(|values| values.last())
        .map(|name| path(name))
        .transpose()
}

pub(crate) fn known(inputs: Vec<String>, outputs: Vec<String>) -> Effects {
    Effects {
        inputs,
        outputs,
        debug_lookup: false,
    }
}

pub(crate) fn inplace(files: Vec<String>) -> Effects {
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
