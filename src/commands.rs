use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use clap::Parser;
use serde_json::json;

use crate::job::{JobSpec, Operation};
use crate::shell::ShellCommand;

fn parse<T: Parser>(shell: &ShellCommand) -> Result<T> {
    Ok(T::try_parse_from(
        std::iter::once(shell.program.as_str()).chain(shell.args.iter().map(String::as_str)),
    )?)
}

fn native(
    kind: &str,
    operation: Operation,
    inputs: Vec<PathBuf>,
    outputs: Vec<PathBuf>,
    cacheable: bool,
    unknown: bool,
    parsed: serde_json::Value,
) -> JobSpec {
    JobSpec {
        kind: kind.into(),
        operation,
        inputs,
        outputs,
        unknown_effects: unknown,
        cacheable,
        parsed,
    }
}

#[derive(Parser)]
#[command(disable_help_flag = true, disable_version_flag = true)]
struct CopyArgs {
    src: PathBuf,
    dst: PathBuf,
}

pub fn copy(shell: &ShellCommand) -> Result<JobSpec> {
    let args: CopyArgs = parse(shell)?;
    Ok(native(
        "cp",
        Operation::Copy {
            src: args.src.clone(),
            dst: args.dst.clone(),
        },
        vec![args.src.clone()],
        vec![args.dst.clone()],
        true,
        false,
        json!({"src":args.src,"dst":args.dst}),
    ))
}

#[derive(Parser)]
#[command(disable_help_flag = true, disable_version_flag = true)]
struct MkdirArgs {
    #[arg(short = 'p', long)]
    parents: bool,
    #[arg(required = true, num_args = 1..)]
    paths: Vec<PathBuf>,
}

pub fn mkdir(shell: &ShellCommand) -> Result<JobSpec> {
    let args: MkdirArgs = parse(shell)?;
    Ok(native(
        "mkdir",
        Operation::Mkdir {
            paths: args.paths.clone(),
            parents: args.parents,
        },
        vec![],
        args.paths.clone(),
        true,
        false,
        json!({"paths":args.paths,"parents":args.parents}),
    ))
}

#[derive(Parser)]
#[command(disable_help_flag = true, disable_version_flag = true)]
struct PathsArgs {
    #[arg(required = true, num_args = 1..)]
    paths: Vec<PathBuf>,
}

pub fn touch(shell: &ShellCommand) -> Result<JobSpec> {
    let args: PathsArgs = parse(shell)?;
    Ok(native(
        "touch",
        Operation::Touch {
            paths: args.paths.clone(),
        },
        args.paths.clone(),
        args.paths.clone(),
        false,
        false,
        json!({"paths":args.paths}),
    ))
}

#[derive(Parser)]
#[command(disable_help_flag = true, disable_version_flag = true)]
struct RemoveArgs {
    #[arg(short = 'f', long)]
    force: bool,
    #[arg(required = true, num_args = 1..)]
    paths: Vec<PathBuf>,
}

pub fn remove(shell: &ShellCommand) -> Result<JobSpec> {
    let args: RemoveArgs = parse(shell)?;
    Ok(native(
        "rm",
        Operation::Remove {
            paths: args.paths.clone(),
            force: args.force,
        },
        args.paths.clone(),
        args.paths.clone(),
        false,
        true,
        json!({"paths":args.paths,"force":args.force}),
    ))
}

pub fn move_file(shell: &ShellCommand) -> Result<JobSpec> {
    let args: CopyArgs = parse(shell)?;
    Ok(native(
        "mv",
        Operation::Move {
            src: args.src.clone(),
            dst: args.dst.clone(),
        },
        vec![args.src.clone()],
        vec![args.src.clone(), args.dst.clone()],
        false,
        true,
        json!({"src":args.src,"dst":args.dst}),
    ))
}

#[derive(Parser)]
#[command(disable_help_flag = true, disable_version_flag = true)]
struct SedArgs {
    #[arg(short = 'n', long = "quiet")]
    quiet: bool,
    #[arg(short = 'e', long = "expression")]
    expressions: Vec<String>,
    #[arg(short = 'f', long = "file")]
    script_files: Vec<PathBuf>,
    #[arg(long = "in-place", num_args = 0..=1, require_equals = true, default_missing_value = "")]
    in_place: Option<String>,
    operands: Vec<String>,
}

pub fn sed(shell: &ShellCommand) -> Result<JobSpec> {
    let mut normalized = shell.clone();
    let mut value = false;
    let mut positional_only = false;
    normalized.args = shell
        .args
        .iter()
        .map(|arg| {
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
        })
        .collect();
    let args: SedArgs = parse(&normalized)?;
    if args.expressions.is_empty() && args.script_files.is_empty() && args.operands.is_empty() {
        bail!("sed requires an expression or script file");
    }
    Ok(native(
        "sed",
        Operation::External,
        vec![],
        vec![],
        false,
        true,
        json!({"quiet":args.quiet,"expressions":args.expressions,"script_files":args.script_files,"in_place":args.in_place,"operands":args.operands}),
    ))
}

#[derive(Parser)]
#[command(disable_help_flag = true, disable_version_flag = true)]
struct CompilerArgs {
    #[arg(short = 'I')]
    includes: Vec<PathBuf>,
    #[arg(short = 'D')]
    defines: Vec<String>,
    #[arg(long = "rusty-optimization", value_parser = ["0", "1", "2", "3", "s", "z", "g", "fast"])]
    optimization: Vec<String>,
    #[arg(long = "rusty-debug", value_parser = ["0", "1", "2", "3"])]
    debug: Vec<String>,
    #[arg(short = 'c')]
    compile_only: bool,
    #[arg(short = 'o')]
    output: Option<PathBuf>,
    #[arg(required = true, num_args = 1..)]
    files: Vec<PathBuf>,
}

fn header_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|s| s.to_str()),
        Some("h" | "H" | "hh" | "hp" | "hpp" | "hxx" | "h++" | "tcc")
    )
}

fn source_file(path: &Path) -> bool {
    header_file(path)
        || matches!(
            path.extension().and_then(|s| s.to_str()),
            Some(
                "c" | "C"
                    | "cc"
                    | "cp"
                    | "cpp"
                    | "CPP"
                    | "cxx"
                    | "c++"
                    | "m"
                    | "mm"
                    | "M"
                    | "s"
                    | "S"
                    | "sx"
                    | "i"
                    | "ii"
                    | "mi"
                    | "mii"
                    | "f"
                    | "for"
                    | "ftn"
                    | "f90"
                    | "f95"
                    | "f03"
                    | "f08"
                    | "F"
                    | "FOR"
                    | "fpp"
                    | "FPP"
                    | "F90"
                    | "F95"
                    | "F03"
                    | "F08"
                    | "adb"
                    | "ads"
                    | "d"
                    | "go"
            )
        )
}

pub fn compiler(shell: &ShellCommand) -> Result<JobSpec> {
    let mut normalized = shell.clone();
    let mut args = Vec::with_capacity(shell.args.len());
    let mut operand_value = false;
    let mut positional_only = false;
    for arg in &shell.args {
        if operand_value {
            args.push(arg.clone());
            operand_value = false;
        } else if positional_only {
            args.push(arg.clone());
        } else if arg == "--" {
            positional_only = true;
            args.push(arg.clone());
        } else if matches!(arg.as_str(), "-I" | "-D" | "-o") {
            operand_value = true;
            args.push(arg.clone());
        } else if let Some(level) = arg.strip_prefix("-O") {
            args.push(format!(
                "--rusty-optimization={}",
                if level.is_empty() { "1" } else { level }
            ));
        } else if let Some(level) = arg.strip_prefix("-g") {
            args.push(format!(
                "--rusty-debug={}",
                if level.is_empty() { "2" } else { level }
            ));
        } else {
            if arg.starts_with('-')
                && arg != "-"
                && arg != "-c"
                && !arg.starts_with("-I")
                && !arg.starts_with("-D")
                && !arg.starts_with("-o")
            {
                bail!("unsupported compiler option: {arg}");
            }
            args.push(arg.clone());
        }
    }
    normalized.args = args;
    let args: CompilerArgs = parse(&normalized)?;
    let sources: Vec<PathBuf> = args
        .files
        .iter()
        .filter(|p| source_file(p))
        .cloned()
        .collect();
    if !args.compile_only && sources.iter().any(|p| header_file(p)) {
        bail!("header compilation requires explicit -c in the supported compiler subset");
    }
    if args.compile_only && args.output.is_some() && sources.len() > 1 {
        bail!("compiler -c with -o requires at most one source file");
    }
    // Response files can hide arbitrary flags and inputs: reject rather than
    // claiming their contents have been parsed by the narrow grammar.
    if args
        .files
        .iter()
        .any(|p| p.as_os_str().to_string_lossy().starts_with('@'))
    {
        bail!("compiler response files are unsupported");
    }
    let outputs = if let Some(output) = &args.output {
        vec![output.clone()]
    } else if args.compile_only {
        sources
            .iter()
            .filter_map(|path| {
                if header_file(path) {
                    let mut output = path.as_os_str().to_owned();
                    output.push(".gch");
                    Some(PathBuf::from(output))
                } else {
                    path.file_name()
                        .map(|name| PathBuf::from(name).with_extension("o"))
                }
            })
            .collect()
    } else {
        vec![PathBuf::from("a.out")]
    };
    let output = if outputs.len() == 1 {
        outputs.first().cloned()
    } else {
        args.output.clone()
    };
    let argv: Vec<&str> = std::iter::once(shell.program.as_str())
        .chain(shell.args.iter().map(String::as_str))
        .collect();
    Ok(native(
        if args.compile_only { "compile" } else { "link" },
        Operation::External,
        args.files.clone(),
        outputs,
        false,
        true,
        json!({"argv":argv,"sources":sources,"output":output,"compile_only":args.compile_only,"includes":args.includes,"defines":args.defines,"optimization":args.optimization,"debug":args.debug}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(program: &str, args: &[&str]) -> ShellCommand {
        ShellCommand {
            program: program.into(),
            args: args.iter().map(|s| (*s).into()).collect(),
            stdin: None,
            stdout: None,
            stderr: None,
            append_stdout: false,
        }
    }

    #[test]
    fn compiler_retains_original_argv_and_sources() {
        let spec = compiler(&command(
            "gcc",
            &[
                "-Iinclude",
                "-DNAME=value",
                "-O2",
                "-g",
                "-c",
                "src/a.c",
                "-o",
                "obj/a.o",
            ],
        ))
        .unwrap();
        assert_eq!(spec.kind, "compile");
        assert_eq!(spec.parsed["sources"], json!(["src/a.c"]));
        assert_eq!(spec.parsed["output"], "obj/a.o");
        assert_eq!(spec.parsed["argv"][3], "-O2");
        assert!(spec.unknown_effects && !spec.cacheable);
    }

    #[test]
    fn compiler_rejects_unknown_options_and_response_files() {
        assert!(compiler(&command("gcc", &["-Wall", "a.c"])).is_err());
        assert!(compiler(&command("gcc", &["-coout.o", "a.c"])).is_err());
        assert!(compiler(&command("gcc", &["@args"])).is_err());
        assert!(compiler(&command("gcc", &["-c", "a.c", "b.c", "-o", "all.o"])).is_err());
        assert!(copy(&command("cp", &["-r", "a", "b"])).is_err());
        assert!(mkdir(&command("mkdir", &["-m", "700", "a"])).is_err());
    }

    #[test]
    fn compiler_default_output_and_link_metadata() {
        let spec = compiler(&command("cc", &["-c", "src/a.c", "src/b.cpp"])).unwrap();
        assert_eq!(
            spec.outputs,
            vec![PathBuf::from("a.o"), PathBuf::from("b.o")]
        );
        let header = compiler(&command("gcc", &["-c", "include/api.h"])).unwrap();
        assert_eq!(header.parsed["sources"], json!(["include/api.h"]));
        assert_eq!(header.parsed["output"], "include/api.h.gch");
        let spec = compiler(&command("cc", &["a.o", "b.c"])).unwrap();
        assert_eq!(spec.kind, "link");
        assert_eq!(spec.parsed["sources"], json!(["b.c"]));
        assert_eq!(spec.parsed["output"], "a.out");
    }
}
