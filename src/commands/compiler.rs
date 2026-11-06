use anyhow::{Result, bail};
use clap::Parser;

use crate::job::{FileAccess, Job, JobData};
use crate::shell::ShellHints;

use super::external::execute_argv;

/// Original compiler argv and only the file metadata understood by this parser.
#[derive(Debug)]
pub struct Compiler {
    pub data: JobData,
    pub argv: Vec<String>,
    pub sources: Vec<String>,
    pub output: Option<String>,
    pub compile_only: bool,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub include_dirs: Vec<String>,
    pub library_paths: Vec<String>,
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
    includes: Vec<String>,
    #[arg(long = "rusty-system-include", allow_hyphen_values = true)]
    system_includes: Vec<String>,
    #[arg(long = "rusty-quote-include", allow_hyphen_values = true)]
    quote_includes: Vec<String>,
    #[arg(long = "rusty-after-include", allow_hyphen_values = true)]
    after_includes: Vec<String>,
    #[arg(short = 'L', allow_hyphen_values = true)]
    library_paths: Vec<String>,
    #[arg(short = 'D')]
    defines: Vec<String>,
    #[arg(long = "rusty-optimization", value_parser = ["0", "1", "2", "3", "s", "z", "g", "fast"])]
    optimization: Vec<String>,
    #[arg(long = "rusty-debug", value_parser = ["0", "1", "2", "3"])]
    debug: Vec<String>,
    #[arg(short = 'c')]
    compile_only: bool,
    #[arg(short = 'o')]
    output: Option<String>,
    #[arg(num_args = 0..)]
    files: Vec<String>,
}

fn filename(path: &str) -> &str {
    if cfg!(windows) {
        path.rsplit(['/', '\\']).next().unwrap_or(path)
    } else {
        path.rsplit('/').next().unwrap_or(path)
    }
}

fn extension(path: &str) -> Option<&str> {
    filename(path)
        .rsplit_once('.')
        .map(|(_, extension)| extension)
}

fn header_file(path: &str) -> bool {
    matches!(
        extension(path),
        Some("h" | "H" | "hh" | "hp" | "hpp" | "hxx" | "h++" | "tcc")
    )
}

fn source_file(path: &str) -> bool {
    header_file(path)
        || matches!(
            extension(path),
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
    /// Extract a recognized subset without modifying argv used for execution.
    pub fn parse(argv: &[String], _shell: &ShellHints) -> Result<Self> {
        let Some((program, original_args)) = argv.split_first() else {
            bail!("compiler requires an executable");
        };
        if program.is_empty() {
            bail!("compiler requires an executable");
        }
        let mut recognized = Vec::with_capacity(original_args.len());
        let mut unparsed_args = Vec::new();
        let mut operand_value = false;
        let mut positional_only = false;
        let mut opaque_operands = false;
        // Unknown options have unknown arity. Bare tokens after one remain
        // opaque until a recognized option boundary or explicit `--`.
        for arg in original_args {
            if operand_value {
                recognized.push(arg.clone());
                operand_value = false;
            } else if arg == "--" && !positional_only {
                positional_only = true;
                opaque_operands = false;
                recognized.push(arg.clone());
            } else if !positional_only && let Some((normalized, value)) = include_option(arg) {
                opaque_operands = false;
                if value.is_empty() {
                    operand_value = true;
                    recognized.push(normalized.into());
                } else {
                    recognized.push(format!("{normalized}={value}"));
                }
            } else if !positional_only && matches!(arg.as_str(), "-I" | "-L" | "-D" | "-o") {
                operand_value = true;
                opaque_operands = false;
                recognized.push(arg.clone());
            } else if !positional_only
                && (arg == "-c"
                    || ["-I", "-L", "-D", "-o"]
                        .iter()
                        .any(|prefix| arg.starts_with(prefix)))
            {
                opaque_operands = false;
                recognized.push(arg.clone());
            } else if !positional_only
                && arg.strip_prefix("-O").is_some_and(|level| {
                    matches!(level, "" | "0" | "1" | "2" | "3" | "s" | "z" | "g" | "fast")
                })
            {
                opaque_operands = false;
                let level = &arg[2..];
                recognized.push(format!(
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
                recognized.push(format!(
                    "--rusty-debug={}",
                    if level.is_empty() { "2" } else { level }
                ));
            } else if !positional_only && matches!(arg.as_str(), "-fPIC" | "-fpic" | "-shared") {
                opaque_operands = false;
            } else if (!positional_only && arg.starts_with('-') && arg != "-")
                || arg.starts_with('@')
            {
                opaque_operands = true;
                unparsed_args.push(arg.clone());
            } else if opaque_operands {
                unparsed_args.push(arg.clone());
            } else {
                recognized.push(arg.clone());
            }
        }
        let args = CompilerArgs::try_parse_from(
            std::iter::once(program.as_str()).chain(recognized.iter().map(String::as_str)),
        )?;
        let metadata_complete = unparsed_args.is_empty();
        if args.files.is_empty() && metadata_complete {
            bail!("compiler requires an input file");
        }
        let sources: Vec<String> = args
            .files
            .iter()
            .filter(|path| source_file(path))
            .cloned()
            .collect();
        if metadata_complete && !args.compile_only && sources.iter().any(|path| header_file(path)) {
            bail!("header compilation requires explicit -c in the supported compiler subset");
        }
        if metadata_complete && args.compile_only && args.output.is_some() && sources.len() > 1 {
            bail!("compiler -c with -o requires at most one source file");
        }
        let outputs = if let Some(output) = &args.output {
            vec![output.clone()]
        } else if !metadata_complete {
            Vec::new()
        } else if args.compile_only {
            sources
                .iter()
                .map(|path| {
                    if header_file(path) {
                        format!("{path}.gch")
                    } else {
                        let name = filename(path);
                        let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
                        format!("{stem}.o")
                    }
                })
                .collect()
        } else {
            vec![if cfg!(windows) { "a.exe" } else { "a.out" }.into()]
        };
        let output = if outputs.len() == 1 {
            outputs.first().cloned()
        } else {
            args.output
        };
        let include_dirs = args
            .includes
            .into_iter()
            .chain(args.system_includes)
            .chain(args.quote_includes)
            .chain(args.after_includes)
            .collect();
        let inputs: Vec<String> = args.files.into_iter().filter(|path| path != "-").collect();
        let mut data = JobData::new(
            inputs
                .iter()
                .map(FileAccess::read)
                .chain(outputs.iter().map(FileAccess::write))
                .collect(),
        );
        // Even recognized invocations can read implicit headers, toolchain files
        // and libraries. A complete argv grammar is not a complete effect model.
        data.unknown_effects = true;
        Ok(Self {
            data,
            argv: argv.to_vec(),
            sources,
            output,
            compile_only: args.compile_only,
            inputs,
            outputs,
            include_dirs,
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
