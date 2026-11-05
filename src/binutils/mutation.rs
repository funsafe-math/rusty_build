use std::path::PathBuf;

use clap::{Arg, ArgAction, ArgMatches, Command};

use super::Effects;
use crate::shell::ShellCommand;

/// Infer only explicit, local file effects. An error requests opaque execution;
/// it never changes which arguments the real binary receives.
pub(super) fn effects(kind: &str, shell: &ShellCommand) -> Result<Effects, String> {
    match kind {
        "objcopy" => objcopy(shell),
        "strip" => strip(shell),
        "ar" => ar(shell),
        "ranlib" => ranlib(shell),
        "elfedit" => elfedit(shell),
        "dwp" => dwp(shell),
        // Even an apparently ordinary source can name arbitrary .include or
        // .incbin files. Likewise, ld treats unrecognized object files as linker
        // scripts, including with -r. Argv alone cannot close either read set.
        "as" => Err("assembly may read .include or .incbin files not present in argv".into()),
        "ld" => Err(
            "link inputs may be scripts with implicit file or library dependencies, even with -r"
                .into(),
        ),
        _ => Err(format!("unsupported binary mutation tool: {kind}")),
    }
}

fn command(name: &'static str) -> Command {
    Command::new(name)
        .disable_help_flag(true)
        .disable_version_flag(true)
}

fn flag(name: &'static str, short: Option<char>) -> Arg {
    let arg = Arg::new(name)
        .long(name)
        .action(ArgAction::SetTrue)
        .overrides_with(name);
    match short {
        Some(short) => arg.short(short),
        None => arg,
    }
}

fn short_flag(name: &'static str, short: char) -> Arg {
    Arg::new(name)
        .short(short)
        .action(ArgAction::SetTrue)
        .overrides_with(name)
}

fn value(name: &'static str, short: Option<char>) -> Arg {
    let arg = Arg::new(name).long(name).action(ArgAction::Append);
    match short {
        Some(short) => arg.short(short),
        None => arg,
    }
}

fn files(min: usize, max: Option<usize>) -> Arg {
    let arg = Arg::new("files").required(true);
    match max {
        Some(max) => arg.num_args(min..=max),
        None => arg.num_args(min..),
    }
}

fn parse(command: Command, shell: &ShellCommand) -> Result<ArgMatches, String> {
    // GNU response files expand before option parsing and can recursively name
    // more files. Do not mistake them for literal positional object names.
    if shell.args.iter().any(|arg| arg.starts_with('@')) {
        return Err("response-file expansion has unknown file effects".into());
    }
    command
        .try_get_matches_from(std::iter::once(&shell.program).chain(shell.args.iter()))
        .map_err(|error| error.to_string())
}

fn paths(matches: &ArgMatches) -> Result<Vec<PathBuf>, String> {
    matches
        .get_many::<String>("files")
        .into_iter()
        .flatten()
        .map(|name| path(name))
        .collect()
}

fn path(name: &str) -> Result<PathBuf, String> {
    if name.is_empty() || name == "-" {
        return Err(
            "empty filenames or implicit standard streams have unknown file effects".into(),
        );
    }
    Ok(PathBuf::from(name))
}

fn last_path(matches: &ArgMatches, name: &str) -> Result<Option<PathBuf>, String> {
    matches
        .get_many::<String>(name)
        .and_then(|values| values.last())
        .map(|name| path(name))
        .transpose()
}

fn known(inputs: Vec<PathBuf>, outputs: Vec<PathBuf>) -> Effects {
    Effects {
        inputs,
        outputs,
        debug_lookup: false,
    }
}

fn inplace(files: Vec<PathBuf>) -> Effects {
    known(files.clone(), files)
}

fn copy_options(mut command: Command) -> Command {
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

fn objcopy(shell: &ShellCommand) -> Result<Effects, String> {
    // objcopy uses -S for strip-all, unlike strip's -s. Keep the
    // shared option's long spelling, assigning that short spelling here.
    let mut parser = copy_options(command("objcopy"))
        .mut_arg("strip-all", |arg| arg.short('S'))
        .arg(flag("strip-debug", Some('g')))
        .arg(value("binary-architecture", Some('B')))
        .arg(value("only-section", Some('j')))
        .arg(value("add-section", None))
        .arg(value("update-section", None))
        .arg(value("dump-section", None))
        .arg(files(1, Some(2)));
    for name in [
        "set-section-flags",
        "set-section-alignment",
        "rename-section",
    ] {
        parser = parser.arg(value(name, None));
    }
    let matches = parse(parser, shell)?;
    let mut operands = paths(&matches)?;
    let output = if operands.len() == 2 {
        operands.pop().expect("two object operands")
    } else {
        operands[0].clone()
    };
    let mut result = known(operands, vec![output]);
    for option in ["add-section", "update-section", "dump-section"] {
        for specification in matches.get_many::<String>(option).into_iter().flatten() {
            let (section, filename) = specification
                .split_once('=')
                .ok_or_else(|| format!("--{option} requires SECTION=FILE"))?;
            if section.is_empty() {
                return Err(format!("--{option} requires a named section"));
            }
            let filename = path(filename)?;
            if option == "dump-section" {
                result.outputs.push(filename);
            } else {
                result.inputs.push(filename);
            }
        }
    }
    Ok(result)
}

fn strip(shell: &ShellCommand) -> Result<Effects, String> {
    let matches = parse(
        copy_options(command("strip"))
            .arg(flag("strip-debug", Some('g')))
            .arg(short_flag("debug-short-S", 'S'))
            .arg(short_flag("debug-short-d", 'd'))
            .arg(value("output-file", Some('o')))
            .arg(files(1, None)),
        shell,
    )?;
    let files = paths(&matches)?;
    match last_path(&matches, "output-file")? {
        Some(output) if files.len() == 1 => Ok(known(files, vec![output])),
        Some(_) => {
            Err("strip -o with multiple inputs has no single understood output mapping".into())
        }
        None => Ok(inplace(files)),
    }
}

fn ar(shell: &ShellCommand) -> Result<Effects, String> {
    // Traditional ar's first word is an operation plus modifiers, optionally
    // prefixed by one dash. Only that positional word admits a leading dash.
    let matches = parse(
        command("ar")
            .arg(
                Arg::new("operation")
                    .required(true)
                    .allow_hyphen_values(true),
            )
            .arg(files(2, None)),
        shell,
    )?;
    let operation = matches
        .get_one::<String>("operation")
        .expect("required operation");
    let operation = operation.strip_prefix('-').unwrap_or(operation);
    let mut replace = false;
    for modifier in operation.chars() {
        match modifier {
            'r' if !replace => replace = true,
            'c' | 's' | 'D' => (),
            _ => return Err("only ar r with c/s/D modifiers has understood file effects; thin, extract, plugin, script and other modes remain opaque".into()),
        }
    }
    if !replace {
        return Err("only archive replacement (ar r) is understood".into());
    }
    let inputs = paths(&matches)?;
    // The existing archive is read as well as rewritten. Member files supply
    // replacements, and insertion creates no files outside the archive itself.
    let outputs = vec![inputs[0].clone()];
    Ok(known(inputs, outputs))
}

fn ranlib(shell: &ShellCommand) -> Result<Effects, String> {
    let matches = parse(
        command("ranlib")
            .arg(flag("enable-deterministic-archives", Some('D')))
            .arg(flag("disable-deterministic-archives", Some('U')))
            .arg(short_flag("touch", 't'))
            .arg(files(1, None)),
        shell,
    )?;
    Ok(inplace(paths(&matches)?))
}

fn elfedit(shell: &ShellCommand) -> Result<Effects, String> {
    let mut parser = command("elfedit").arg(files(1, None));
    for name in [
        "input-mach",
        "output-mach",
        "input-type",
        "output-type",
        "input-osabi",
        "output-osabi",
        "input-abiversion",
        "output-abiversion",
    ] {
        parser = parser.arg(value(name, None));
    }
    let matches = parse(parser, shell)?;
    Ok(inplace(paths(&matches)?))
}

fn dwp(shell: &ShellCommand) -> Result<Effects, String> {
    // -e discovers .dwo inputs through an executable's debug information;
    // explicit .dwo inputs and an explicit output do not require that lookup.
    let matches = parse(
        command("dwp")
            .arg(value("output", Some('o')).required(true))
            .arg(flag("verbose", Some('v')))
            .arg(files(1, None)),
        shell,
    )?;
    let output = last_path(&matches, "output")?.expect("required output");
    Ok(known(paths(&matches)?, vec![output]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn infer(text: &str) -> Result<Effects, String> {
        let shell = crate::shell::parse(text).unwrap().unwrap();
        effects(&shell.program, &shell)
    }

    fn paths_of(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn objcopy_explicit_and_inplace_outputs() {
        let effect = infer("objcopy --only-section=.text input.o output.o").unwrap();
        assert_eq!(effect.inputs, paths_of(&["input.o"]));
        assert_eq!(effect.outputs, paths_of(&["output.o"]));
        let effect = infer("objcopy --only-section=.text input.o").unwrap();
        assert_eq!(effect.inputs, paths_of(&["input.o"]));
        assert_eq!(effect.outputs, paths_of(&["input.o"]));
    }

    #[test]
    fn objcopy_exposes_all_section_file_effects() {
        let effect = infer("objcopy --add-section=.one=one.bin --update-section .two=two.bin --dump-section=.text=text.bin --dump-section .data=data.bin input.o output.o").unwrap();
        assert_eq!(effect.inputs, paths_of(&["input.o", "one.bin", "two.bin"]));
        assert_eq!(
            effect.outputs,
            paths_of(&["output.o", "text.bin", "data.bin"])
        );
        let effect = infer("objcopy --dump-section=.text=dump.bin input.o").unwrap();
        assert_eq!(effect.outputs, paths_of(&["input.o", "dump.bin"]));
    }

    #[test]
    fn strip_output_and_inplace_hazards() {
        let effect = infer("strip --strip-debug first.o second.o").unwrap();
        assert_eq!(effect.inputs, paths_of(&["first.o", "second.o"]));
        assert_eq!(effect.outputs, effect.inputs);
        let effect = infer("strip --strip-debug -o stripped.o input.o").unwrap();
        assert_eq!(effect.inputs, paths_of(&["input.o"]));
        assert_eq!(effect.outputs, paths_of(&["stripped.o"]));
        assert!(infer("strip -o out.o a.o b.o").is_err());
    }

    #[test]
    fn archives_are_read_modify_write() {
        for text in ["ar rcs lib.a one.o two.o", "ar -rcs lib.a one.o two.o"] {
            let effect = infer(text).unwrap();
            assert_eq!(effect.inputs, paths_of(&["lib.a", "one.o", "two.o"]));
            assert_eq!(effect.outputs, paths_of(&["lib.a"]));
        }
        let effect = infer("ranlib first.a second.a").unwrap();
        assert_eq!(effect.inputs, paths_of(&["first.a", "second.a"]));
        assert_eq!(effect.outputs, effect.inputs);
        let effect = infer("elfedit --output-osabi GNU input.o").unwrap();
        assert_eq!(effect.inputs, paths_of(&["input.o"]));
        assert_eq!(effect.outputs, effect.inputs);
    }

    #[test]
    fn dwp_requires_explicit_inputs_and_output() {
        let effect = infer("dwp -o package.dwp one.dwo two.dwo").unwrap();
        assert_eq!(effect.inputs, paths_of(&["one.dwo", "two.dwo"]));
        assert_eq!(effect.outputs, paths_of(&["package.dwp"]));
        assert!(infer("dwp -e executable -o package.dwp").is_err());
        assert!(infer("dwp one.dwo").is_err());
    }

    #[test]
    fn opaque_arguments_request_fallback() {
        for text in [
            "objcopy --unknown input.o output.o",
            "objcopy --add-gnu-debuglink=debug.o input.o",
            "objcopy --add-section=.text input.o output.o",
            "strip --plugin=plugin.so input.o",
            "ranlib --plugin=plugin.so lib.a",
            "ar rcT lib.a input.o",
            "ar x lib.a member.o",
            "ar -M script",
            "ar rcs --plugin=plugin.so lib.a input.o",
            "elfedit --unknown input.o",
            "objcopy @options input.o output.o",
            "as -o input.o input.s",
            "ld -r -o combined.o one.o two.o",
            "ld -r -T layout.ld -o combined.o one.o",
        ] {
            assert!(
                infer(text).is_err(),
                "unexpected precise effects for {text}"
            );
        }
    }
}
