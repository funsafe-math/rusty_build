use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use clap::Parser;
use serde::{Deserialize, Serialize};

use crate::job::{ExecutionContext, Job};
use crate::registry::command_pattern;
use crate::shell::ShellCommand;

use super::external::execute_argv;

/// Compiler arguments plus the conservatively inferred portion of their effects.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Compiler {
    pub argv: Vec<String>,
    pub sources: Vec<PathBuf>,
    pub output: Option<PathBuf>,
    pub compile_only: bool,
    pub inputs: Vec<PathBuf>,
    pub outputs: Vec<PathBuf>,
    pub includes: Vec<PathBuf>,
    pub library_paths: Vec<PathBuf>,
    pub defines: Vec<String>,
    pub optimization: Vec<String>,
    pub debug: Vec<String>,
    pub unparsed_args: Vec<String>,
    pub metadata_complete: bool,
}

#[derive(Parser)]
#[command(disable_help_flag = true, disable_version_flag = true)]
struct CompilerArgs {
    #[arg(short = 'I', allow_hyphen_values = true)]
    includes: Vec<PathBuf>,
    #[arg(long = "rusty-system-include", allow_hyphen_values = true)]
    system_includes: Vec<PathBuf>,
    #[arg(long = "rusty-quote-include", allow_hyphen_values = true)]
    quote_includes: Vec<PathBuf>,
    #[arg(long = "rusty-after-include", allow_hyphen_values = true)]
    after_includes: Vec<PathBuf>,
    #[arg(short = 'L', allow_hyphen_values = true)]
    library_paths: Vec<PathBuf>,
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
    #[arg(num_args = 0..)]
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

fn include_option(arg: &str) -> Option<(&'static str, &str)> {
    [
        ("-isystem", "--rusty-system-include"),
        ("-iquote", "--rusty-quote-include"),
        ("-idirafter", "--rusty-after-include"),
    ]
    .into_iter()
    .find_map(|(flag, normalized)| arg.strip_prefix(flag).map(|value| (normalized, value)))
}

impl Compiler {
    /// Infer metadata without changing any argument passed to the compiler.
    pub fn from_argv(argv: Vec<String>) -> Result<Self> {
        let Some((program, original_args)) = argv.split_first() else {
            bail!("compiler requires an executable");
        };
        if program.is_empty() {
            bail!("compiler requires an executable");
        }
        let mut args = Vec::with_capacity(original_args.len());
        let mut unparsed_args = Vec::new();
        let mut operand_value = false;
        let mut positional_only = false;
        let mut opaque_operands = false;
        // Partition lexically; clap owns the recognized grammar. The arity of
        // unknown flags is unknowable, so their bare operands stay opaque until
        // another option boundary. Original argv always remains untouched.
        for arg in original_args {
            if operand_value {
                args.push(arg.clone());
                operand_value = false;
            } else if arg == "--" && !positional_only {
                positional_only = true;
                opaque_operands = false;
                args.push(arg.clone());
            } else if !positional_only && let Some((normalized, value)) = include_option(arg) {
                opaque_operands = false;
                if value.is_empty() {
                    operand_value = true;
                    args.push(normalized.into());
                } else {
                    args.push(format!("{normalized}={value}"));
                }
            } else if !positional_only && matches!(arg.as_str(), "-I" | "-L" | "-D" | "-o") {
                operand_value = true;
                opaque_operands = false;
                args.push(arg.clone());
            } else if !positional_only
                && (arg == "-c"
                    || ["-I", "-L", "-D", "-o"]
                        .iter()
                        .any(|prefix| arg.starts_with(prefix)))
            {
                opaque_operands = false;
                args.push(arg.clone());
            } else if !positional_only
                && arg.strip_prefix("-O").is_some_and(|level| {
                    matches!(level, "" | "0" | "1" | "2" | "3" | "s" | "z" | "g" | "fast")
                })
            {
                opaque_operands = false;
                let level = &arg[2..];
                args.push(format!(
                    "--rusty-optimization={}",
                    if level.is_empty() { "1" } else { level }
                ));
            } else if !positional_only
                && arg
                    .strip_prefix("-g")
                    .is_some_and(|level| matches!(level, "" | "0" | "1" | "2" | "3"))
            {
                opaque_operands = false;
                let level = &arg[2..];
                args.push(format!(
                    "--rusty-debug={}",
                    if level.is_empty() { "2" } else { level }
                ));
            } else if !positional_only && matches!(arg.as_str(), "-fPIC" | "-fpic" | "-shared") {
                // These switches have no operands or extra file effects. Keep
                // their exact spelling in argv, not clap's metadata grammar.
                opaque_operands = false;
            } else if (!positional_only && arg.starts_with('-') && arg != "-")
                || arg.starts_with('@')
            {
                opaque_operands = true;
                unparsed_args.push(arg.clone());
            } else if opaque_operands {
                unparsed_args.push(arg.clone());
            } else {
                args.push(arg.clone());
            }
        }
        let args = CompilerArgs::try_parse_from(
            std::iter::once(program.as_str()).chain(args.iter().map(String::as_str)),
        )?;
        let metadata_complete = unparsed_args.is_empty();
        if args.files.is_empty() && metadata_complete {
            bail!("compiler requires an input file");
        }
        let sources: Vec<PathBuf> = args
            .files
            .iter()
            .filter(|p| source_file(p))
            .cloned()
            .collect();
        if metadata_complete && !args.compile_only && sources.iter().any(|p| header_file(p)) {
            bail!("header compilation requires explicit -c in the supported compiler subset");
        }
        if metadata_complete && args.compile_only && args.output.is_some() && sources.len() > 1 {
            bail!("compiler -c with -o requires at most one source file");
        }
        let outputs = if let Some(output) = &args.output {
            vec![output.clone()]
        } else if !metadata_complete {
            // Partial metadata cannot predict preprocessing or custom outputs.
            vec![]
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
            args.output
        };
        let includes = args
            .includes
            .into_iter()
            .chain(args.system_includes)
            .chain(args.quote_includes)
            .chain(args.after_includes)
            .collect();
        Ok(Self {
            argv,
            sources,
            output,
            compile_only: args.compile_only,
            inputs: args.files,
            outputs,
            includes,
            library_paths: args.library_paths,
            defines: args.defines,
            optimization: args.optimization,
            debug: args.debug,
            unparsed_args,
            metadata_complete,
        })
    }
}

impl Job for Compiler {
    const TYPE: &'static str = "compiler";

    fn patterns() -> Vec<String> {
        vec![command_pattern(
            r"(?:cc|c\+\+|gcc|g\+\+|clang|clang\+\+)(?:-[0-9]+(?:\.[0-9]+)*)?",
        )]
    }

    fn parse(shell: &ShellCommand) -> Result<Self> {
        Self::from_argv(
            std::iter::once(shell.program.clone())
                .chain(shell.args.iter().cloned())
                .collect(),
        )
    }

    fn kind(&self) -> &str {
        if self.compile_only { "compile" } else { "link" }
    }

    fn inputs(&self) -> impl Iterator<Item = &Path> {
        self.inputs.iter().map(PathBuf::as_path)
    }

    fn outputs(&self) -> impl Iterator<Item = &Path> {
        self.outputs.iter().map(PathBuf::as_path)
    }

    fn unknown_effects(&self) -> bool {
        true
    }

    fn execute(&self, ctx: &ExecutionContext<'_>) -> Result<()> {
        execute_argv(&self.argv, ctx)
    }

    // Default batching launches each compiler independently, using its typed argv.
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compiler(args: &[&str]) -> Compiler {
        Compiler::from_argv(
            std::iter::once("cc")
                .chain(args.iter().copied())
                .map(str::to_owned)
                .collect(),
        )
        .unwrap()
    }

    #[test]
    fn compiler_retains_original_argv_and_sources() {
        let job = compiler(&[
            "-Iinclude",
            "-DNAME=value",
            "-O2",
            "-g",
            "-c",
            "src/a.c",
            "-o",
            "obj/a.o",
        ]);
        assert_eq!(job.kind(), "compile");
        assert_eq!(job.sources, [PathBuf::from("src/a.c")]);
        assert_eq!(job.output.as_deref(), Some(Path::new("obj/a.o")));
        assert_eq!(job.argv[3], "-O2");
        assert!(job.unknown_effects() && !job.cacheable());
    }

    #[test]
    fn compiler_default_output_and_link_metadata() {
        let job = compiler(&["-c", "src/a.c", "src/b.cpp"]);
        assert_eq!(job.outputs, [PathBuf::from("a.o"), PathBuf::from("b.o")]);
        let header = compiler(&["-c", "include/api.h"]);
        assert_eq!(header.sources, [PathBuf::from("include/api.h")]);
        assert_eq!(
            header.output.as_deref(),
            Some(Path::new("include/api.h.gch"))
        );
        let link = compiler(&["a.o", "b.c"]);
        assert_eq!(link.kind(), "link");
        assert_eq!(link.sources, [PathBuf::from("b.c")]);
        assert_eq!(link.output.as_deref(), Some(Path::new("a.out")));
    }

    #[test]
    fn unknown_flags_and_operands_remain_opaque() {
        let job = compiler(&[
            "-include",
            "generated.c",
            "-Wall",
            "-c",
            "main.c",
            "-o",
            "main.o",
        ]);
        assert_eq!(job.unparsed_args, ["-include", "generated.c", "-Wall"]);
        assert_eq!(job.sources, [PathBuf::from("main.c")]);
        assert!(!job.metadata_complete);
        assert!(compiler(&["@args.rsp"]).outputs.is_empty());
    }

    #[test]
    fn include_categories_and_library_paths_keep_existing_metadata_order() {
        let job = compiler(&[
            "-isystemsystem",
            "-Ilocal",
            "-iquote",
            "quoted",
            "-idirafterafter",
            "-L",
            "lib",
            "-c",
            "main.c",
        ]);
        assert_eq!(
            job.includes,
            ["local", "system", "quoted", "after"].map(PathBuf::from)
        );
        assert_eq!(job.library_paths, [PathBuf::from("lib")]);
        assert!(job.metadata_complete);
    }

    #[test]
    fn arbitrary_program_identity_is_preserved_in_typed_serialization() {
        for program in ["xcruncc", "mycompiler", "/opt/tool chain/custom frontend"] {
            let argv = [program, "-c", "main.c", "-o", "main.o"]
                .map(str::to_owned)
                .to_vec();
            let job = Compiler::from_argv(argv.clone()).unwrap();
            assert_eq!(job.argv, argv);
            assert_eq!(serde_json::to_value(&job).unwrap()["argv"][0], program);
            let restored: Compiler =
                serde_json::from_value(serde_json::to_value(&job).unwrap()).unwrap();
            assert_eq!(restored.argv, argv);
            let shell = ShellCommand {
                program: program.into(),
                args: argv[1..].to_vec(),
                stdin: None,
                stdout: None,
                stderr: None,
                append_stdout: false,
                noop_fallback: false,
            };
            assert_eq!(Compiler::parse(&shell).unwrap().argv, argv);
        }
    }

    #[test]
    fn execution_uses_transformed_argv_without_replacing_source_provenance() {
        use crate::input::SourceCommand;
        use crate::job::JobRecord;

        let root = tempfile::tempdir().unwrap();
        let source = SourceCommand {
            line: 7,
            text: "false".into(),
            target: None,
        };
        let shell = crate::shell::parse(&source.text).unwrap();
        let job = Compiler::from_argv(
            ["sh", "-c", "printf typed > typed-output"]
                .map(str::to_owned)
                .to_vec(),
        )
        .unwrap();
        let record = JobRecord::new(source, shell, job).unwrap();
        let ctx = ExecutionContext {
            cwd: root.path(),
            record: &record,
        };
        record.typed::<Compiler>().unwrap().execute(&ctx).unwrap();
        assert_eq!(
            std::fs::read(root.path().join("typed-output")).unwrap(),
            b"typed"
        );
        assert_eq!(record.source.text, "false");
    }
}
