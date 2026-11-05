//! Deliberately partial GNU inspection grammars, not replacements for tool argv.
//!
//! Unknown options (including source, debug-follow, plugin and response-file
//! modes) retain opaque execution through the facade. Named objects are precise
//! only within its guarded regular-file/non-thin/non-plugin artifact domain.
//! Address lookup and ordinary disassembly retain an external-debug barrier.
//! Redirection effects belong to the facade; positional addresses and symbols
//! are data, never filesystem inputs.

use std::path::PathBuf;

use clap::{Args, Parser};

use super::Effects;
use crate::shell::ShellCommand;

fn parse<T: Parser>(shell: &ShellCommand) -> Result<T, String> {
    // GNU expandargv expands these even after `--`, before option parsing.
    if shell.args.iter().any(|arg| arg.starts_with('@')) {
        return Err("response files can introduce untracked arguments and inputs".into());
    }
    T::try_parse_from(
        std::iter::once(shell.program.as_str()).chain(shell.args.iter().map(String::as_str)),
    )
    .map_err(|error| error.to_string())
}

fn objects(mut files: Vec<PathBuf>, default: Option<&str>) -> Result<Effects, String> {
    if files.is_empty() {
        let Some(default) = default else {
            return Err("this inspection mode needs explicit object files".into());
        };
        files.push(default.into());
    }
    if files.iter().any(|file| file.as_os_str() == "-") {
        return Err("dash operands have tool-specific stream semantics".into());
    }
    Ok(Effects {
        inputs: files,
        outputs: Vec::new(),
        debug_lookup: false,
    })
}

fn require_stdin(shell: &ShellCommand) -> Result<(), String> {
    if shell.stdin.is_none() {
        return Err("this mode reads stdin without an explicit input redirection".into());
    }
    Ok(())
}

#[derive(Args)]
struct Demangle {
    // Optional values must use `=`: bare -C must not swallow an object filename.
    #[arg(short = 'C', long = "demangle", num_args = 0..=1, require_equals = true, default_missing_value = "auto")]
    _style: Option<String>,
    #[arg(long = "no-demangle")]
    _no_demangle: bool,
    #[arg(long = "recurse-limit", alias = "recursion-limit")]
    _recurse_limit: bool,
    #[arg(long = "no-recurse-limit", alias = "no-recursion-limit")]
    _no_recurse_limit: bool,
}

#[derive(Parser)]
#[command(
    disable_help_flag = true,
    disable_version_flag = true,
    args_override_self = true
)]
struct NmArgs {
    #[command(flatten)]
    _demangle: Demangle,
    #[arg(short = 'a', long = "debug-syms")]
    _debug_syms: bool,
    #[arg(short = 'A', long = "print-file-name", short_alias = 'o')]
    _print_file_name: bool,
    #[arg(short = 'B')]
    _bsd: bool,
    #[arg(short = 'D', long = "dynamic")]
    _dynamic: bool,
    #[arg(short = 'e')]
    _ignored: bool,
    #[arg(short = 'f', long = "format", value_parser = ["bsd", "sysv", "posix", "just-symbols"])]
    _format: Option<String>,
    #[arg(short = 'g', long = "extern-only")]
    _extern_only: bool,
    #[arg(short = 'j', long = "just-symbols")]
    _just_symbols: bool,
    #[arg(short = 'n', long = "numeric-sort", short_alias = 'v')]
    _numeric_sort: bool,
    #[arg(short = 'p', long = "no-sort")]
    _no_sort: bool,
    #[arg(short = 'P', long = "portability")]
    _portability: bool,
    #[arg(short = 'r', long = "reverse-sort")]
    _reverse_sort: bool,
    #[arg(short = 'S', long = "print-size")]
    _print_size: bool,
    #[arg(short = 's', long = "print-armap")]
    _print_armap: bool,
    #[arg(short = 't', long = "radix", value_parser = ["d", "o", "x"])]
    _radix: Option<String>,
    #[arg(short = 'u', long = "undefined-only")]
    _undefined_only: bool,
    #[arg(short = 'U', long = "defined-only")]
    _defined_only: bool,
    #[arg(short = 'W', long = "no-weak")]
    _no_weak: bool,
    #[arg(long = "quiet")]
    _quiet: bool,
    #[arg(long = "size-sort")]
    _size_sort: bool,
    #[arg(long = "special-syms")]
    _special_syms: bool,
    #[arg(long = "synthetic")]
    _synthetic: bool,
    #[arg(long = "without-symbol-versions")]
    _without_versions: bool,
    files: Vec<PathBuf>,
}

#[derive(Parser)]
#[command(
    disable_help_flag = true,
    disable_version_flag = true,
    args_override_self = true
)]
struct ObjdumpArgs {
    #[command(flatten)]
    _demangle: Demangle,
    #[arg(short = 'a', long = "archive-headers")]
    _archive_headers: bool,
    #[arg(short = 'f', long = "file-headers")]
    _file_headers: bool,
    #[arg(short = 'p', long = "private-headers")]
    _private_headers: bool,
    #[arg(short = 'h', long = "section-headers", alias = "headers")]
    _section_headers: bool,
    #[arg(short = 'x', long = "all-headers")]
    _all_headers: bool,
    #[arg(short = 'd', long = "disassemble", num_args = 0..=1, require_equals = true, default_missing_value = "")]
    disassemble: Option<String>,
    #[arg(short = 'D', long = "disassemble-all")]
    disassemble_all: bool,
    // This is a lookup-control flag, not permission to parse arbitrary DWARF
    // selectors. Source/line-number modes are still rejected independently.
    #[arg(long = "dwarf", value_parser = ["no-follow-links"], require_equals = true)]
    no_follow_links: Option<String>,
    #[arg(short = 's', long = "full-contents")]
    _full_contents: bool,
    #[arg(short = 'Z', long = "decompress")]
    _decompress: bool,
    #[arg(short = 't', long = "syms")]
    _syms: bool,
    #[arg(short = 'T', long = "dynamic-syms")]
    _dynamic_syms: bool,
    #[arg(short = 'r', long = "reloc")]
    _reloc: bool,
    #[arg(short = 'R', long = "dynamic-reloc")]
    _dynamic_reloc: bool,
    #[arg(short = 'j', long = "section")]
    _sections: Vec<String>,
    #[arg(short = 'm', long = "architecture")]
    _architecture: Option<String>,
    #[arg(short = 'w', long = "wide")]
    _wide: bool,
    #[arg(short = 'z', long = "disassemble-zeroes")]
    _zeroes: bool,
    #[arg(short = 'F', long = "file-offsets")]
    _file_offsets: bool,
    #[arg(long = "start-address")]
    _start: Option<String>,
    #[arg(long = "stop-address")]
    _stop: Option<String>,
    #[arg(long = "adjust-vma")]
    _adjust_vma: Option<String>,
    #[arg(long = "no-addresses")]
    _no_addresses: bool,
    #[arg(long = "prefix-addresses")]
    _prefix_addresses: bool,
    #[arg(long = "show-raw-insn")]
    _raw: bool,
    #[arg(long = "no-show-raw-insn")]
    _no_raw: bool,
    #[arg(long = "insn-width")]
    _insn_width: Option<usize>,
    #[arg(long = "show-all-symbols")]
    _show_all_symbols: bool,
    #[arg(long = "special-syms")]
    _special_syms: bool,
    files: Vec<PathBuf>,
}

#[derive(Parser)]
#[command(
    disable_help_flag = true,
    disable_version_flag = true,
    args_override_self = true
)]
struct ReadelfArgs {
    #[command(flatten)]
    _demangle: Demangle,
    #[arg(short = 'a', long = "all")]
    _all: bool,
    #[arg(short = 'h', long = "file-header")]
    _file_header: bool,
    #[arg(short = 'l', long = "program-headers", alias = "segments")]
    _program_headers: bool,
    #[arg(short = 'S', long = "section-headers", alias = "sections")]
    _section_headers: bool,
    #[arg(short = 'g', long = "section-groups")]
    _section_groups: bool,
    #[arg(short = 't', long = "section-details")]
    _section_details: bool,
    #[arg(short = 'e', long = "headers")]
    _headers: bool,
    #[arg(short = 's', long = "syms", alias = "symbols")]
    _syms: bool,
    #[arg(long = "dyn-syms")]
    _dyn_syms: bool,
    #[arg(long = "lto-syms")]
    _lto_syms: bool,
    #[arg(short = 'n', long = "notes")]
    _notes: bool,
    #[arg(short = 'r', long = "relocs")]
    _relocs: bool,
    #[arg(short = 'u', long = "unwind")]
    _unwind: bool,
    #[arg(short = 'd', long = "dynamic")]
    _dynamic: bool,
    #[arg(short = 'V', long = "version-info")]
    _version_info: bool,
    #[arg(short = 'A', long = "arch-specific")]
    _arch_specific: bool,
    #[arg(short = 'c', long = "archive-index")]
    _archive_index: bool,
    #[arg(short = 'D', long = "use-dynamic")]
    _use_dynamic: bool,
    #[arg(short = 'L', long = "lint", alias = "enable-checks")]
    _lint: bool,
    #[arg(short = 'x', long = "hex-dump")]
    _hex_dump: Vec<String>,
    #[arg(short = 'p', long = "string-dump")]
    _string_dump: Vec<String>,
    #[arg(short = 'R', long = "relocated-dump")]
    _relocated_dump: Vec<String>,
    #[arg(short = 'z', long = "decompress")]
    _decompress: bool,
    #[arg(short = 'I', long = "histogram")]
    _histogram: bool,
    #[arg(short = 'W', long = "wide")]
    _wide: bool,
    #[arg(short = 'T', long = "silent-truncation")]
    _silent_truncation: bool,
    files: Vec<PathBuf>,
}

#[derive(Parser)]
#[command(
    disable_help_flag = true,
    disable_version_flag = true,
    args_override_self = true
)]
struct SizeArgs {
    #[arg(short = 'A')]
    _sysv: bool,
    #[arg(short = 'B')]
    _berkeley: bool,
    #[arg(short = 'G')]
    _gnu: bool,
    #[arg(long = "format", value_parser = ["sysv", "berkeley", "gnu"])]
    _format: Option<String>,
    #[arg(short = 'o')]
    _octal: bool,
    #[arg(short = 'd')]
    _decimal: bool,
    #[arg(short = 'x')]
    _hex: bool,
    #[arg(long = "radix", value_parser = ["8", "10", "16"])]
    _radix: Option<String>,
    #[arg(short = 't', long = "totals")]
    _totals: bool,
    #[arg(short = 'f')]
    _ignored: bool,
    #[arg(long = "common")]
    _common: bool,
    files: Vec<PathBuf>,
}

#[derive(Parser)]
#[command(
    disable_help_flag = true,
    disable_version_flag = true,
    args_override_self = true
)]
struct StringsArgs {
    #[arg(short = 'a', long = "all")]
    _all: bool,
    #[arg(short = 'd', long = "data")]
    _data: bool,
    #[arg(short = 'f', long = "print-file-name")]
    _print_file_name: bool,
    #[arg(short = 'n', long = "bytes")]
    _bytes: Option<usize>,
    #[arg(short = 't', long = "radix", value_parser = ["o", "d", "x"])]
    _radix: Option<String>,
    #[arg(short = 'w', long = "include-all-whitespace")]
    _whitespace: bool,
    #[arg(short = 'o')]
    _octal: bool,
    #[arg(short = 'e', long = "encoding", value_parser = ["s", "S", "b", "l", "B", "L"])]
    _encoding: Option<String>,
    #[arg(short = 'U', long = "unicode")]
    _unicode: Option<String>,
    #[arg(short = 's', long = "output-separator")]
    _separator: Option<String>,
    files: Vec<PathBuf>,
}

#[derive(Parser)]
#[command(
    disable_help_flag = true,
    disable_version_flag = true,
    args_override_self = true
)]
struct Addr2lineArgs {
    #[arg(short = 'e', long = "exe", default_value = "a.out")]
    exe: PathBuf,
    #[arg(short = 'a', long = "addresses")]
    _addresses: bool,
    #[arg(short = 'i', long = "inlines")]
    _inlines: bool,
    #[arg(short = 'j', long = "section")]
    _section: Option<String>,
    #[arg(short = 'p', long = "pretty-print")]
    _pretty: bool,
    #[arg(short = 's', long = "basenames")]
    _basenames: bool,
    #[arg(short = 'f', long = "functions")]
    _functions: bool,
    #[arg(short = 'C', long = "demangle", num_args = 0..=1, require_equals = true, default_missing_value = "auto")]
    _demangle: Option<String>,
    #[arg(short = 'R', long = "recurse-limit")]
    _recurse_limit: bool,
    #[arg(short = 'r', long = "no-recurse-limit")]
    _no_recurse_limit: bool,
    addresses: Vec<String>,
}

#[derive(Parser)]
#[command(
    disable_help_flag = true,
    disable_version_flag = true,
    args_override_self = true
)]
struct FiltArgs {
    #[arg(short = '_', long = "strip-underscore")]
    _strip_underscore: bool,
    #[arg(short = 'n', long = "no-strip-underscore")]
    _no_strip_underscore: bool,
    #[arg(short = 'p', long = "no-params")]
    _no_params: bool,
    #[arg(short = 'i', long = "no-verbose")]
    _no_verbose: bool,
    #[arg(short = 'R', long = "recurse-limit")]
    _recurse_limit: bool,
    #[arg(short = 'r', long = "no-recurse-limit")]
    _no_recurse_limit: bool,
    #[arg(short = 't', long = "types")]
    _types: bool,
    #[arg(short = 's', long = "format", value_parser = ["none", "auto", "gnu-v3", "java", "gnat", "dlang", "rust"])]
    _format: Option<String>,
    symbols: Vec<String>,
}

// Only the LLVM readobj forms below are supported. In particular this does not
// assume that GNU readelf's short options have LLVM readobj semantics.
#[derive(Parser)]
#[command(
    disable_help_flag = true,
    disable_version_flag = true,
    args_override_self = true
)]
struct ReadobjArgs {
    #[arg(long = "file-headers")]
    _file_headers: bool,
    #[arg(long = "sections")]
    _sections: bool,
    #[arg(long = "section-data")]
    _section_data: bool,
    #[arg(long = "section-relocations")]
    _section_relocations: bool,
    #[arg(long = "section-symbols")]
    _section_symbols: bool,
    #[arg(long = "symbols")]
    _symbols: bool,
    #[arg(long = "dyn-symbols")]
    _dyn_symbols: bool,
    #[arg(long = "relocations")]
    _relocations: bool,
    #[arg(long = "notes")]
    _notes: bool,
    #[arg(long = "program-headers")]
    _program_headers: bool,
    #[arg(long = "dynamic-table")]
    _dynamic_table: bool,
    #[arg(long = "needed-libs")]
    _needed_libs: bool,
    #[arg(long = "unwind")]
    _unwind: bool,
    #[arg(long = "elf-output-style", value_parser = ["LLVM", "GNU", "JSON"])]
    _elf_output_style: Option<String>,
    files: Vec<PathBuf>,
}

pub(super) fn effects(kind: &str, shell: &ShellCommand) -> Result<Effects, String> {
    match kind {
        "nm" => objects(parse::<NmArgs>(shell)?.files, Some("a.out")),
        "objdump" => {
            let args = parse::<ObjdumpArgs>(shell)?;
            // Requiring explicit files avoids depending on version-specific
            // usage/default behavior for objdump invocations without operands.
            let mut effects = objects(args.files, None)?;
            effects.debug_lookup = (args.disassemble.is_some() || args.disassemble_all)
                && args.no_follow_links.is_none();
            Ok(effects)
        }
        "readelf" => objects(parse::<ReadelfArgs>(shell)?.files, None),
        "size" => objects(parse::<SizeArgs>(shell)?.files, Some("a.out")),
        "strings" => {
            let args = parse::<StringsArgs>(shell)?;
            if args.files.is_empty() {
                require_stdin(shell)?;
                Ok(Effects {
                    inputs: Vec::new(),
                    outputs: Vec::new(),
                    debug_lookup: false,
                })
            } else {
                objects(args.files, None)
            }
        }
        "addr2line" => {
            let args = parse::<Addr2lineArgs>(shell)?;
            if args.addresses.is_empty() {
                require_stdin(shell)?;
            }
            let mut effects = objects(vec![args.exe], None)?;
            effects.debug_lookup = true;
            Ok(effects)
        }
        "c++filt" => {
            let args = parse::<FiltArgs>(shell)?;
            if args.symbols.is_empty() {
                require_stdin(shell)?;
            }
            Ok(Effects {
                inputs: Vec::new(),
                outputs: Vec::new(),
                debug_lookup: false,
            })
        }
        "readobj" => objects(parse::<ReadobjArgs>(shell)?.files, None),
        _ => Err(format!("no inspection grammar for {kind}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(program: &str, args: &[&str], stdin: Option<&str>) -> ShellCommand {
        ShellCommand {
            program: program.into(),
            args: args.iter().map(|arg| (*arg).into()).collect(),
            stdin: stdin.map(PathBuf::from),
            stdout: None,
            stderr: None,
            append_stdout: false,
        }
    }

    fn assert_inputs(kind: &str, args: &[&str], expected: &[&str]) {
        let result = effects(kind, &command(kind, args, None)).unwrap();
        assert_eq!(
            result.inputs,
            expected.iter().map(PathBuf::from).collect::<Vec<_>>()
        );
        assert!(result.outputs.is_empty());
    }

    #[test]
    fn common_object_inspections_have_only_named_inputs() {
        assert_inputs(
            "nm",
            &["-n", "--defined-only", "first.o", "second.o"],
            &["first.o", "second.o"],
        );
        assert_inputs("objdump", &["-h", "object.o"], &["object.o"]);
        assert_inputs("objdump", &["-d", "object.o"], &["object.o"]);
        assert_inputs("objdump", &["-at", "object.o"], &["object.o"]);
        assert_inputs("readelf", &["-h", "object.o"], &["object.o"]);
        assert_inputs("readelf", &["-a", "object.o"], &["object.o"]);
        assert_inputs("size", &["-Ax", "object.o"], &["object.o"]);
        assert_inputs("strings", &["-n", "12", "object.o"], &["object.o"]);
        assert_inputs("readobj", &["--file-headers", "object.o"], &["object.o"]);
    }

    #[test]
    fn option_values_and_positional_data_are_not_paths() {
        assert_inputs("nm", &["-C", "--format=posix", "object.o"], &["object.o"]);
        assert_inputs(
            "objdump",
            &["--disassemble=symbol", "-j", ".text", "object.o"],
            &["object.o"],
        );
        assert_inputs("readelf", &["-p", ".comment", "object.o"], &["object.o"]);
        assert_inputs(
            "addr2line",
            &[
                "-f", "-C", "-i", "-p", "-a", "-s", "-j", ".text", "-e", "app", "0", "0x1234",
            ],
            &["app"],
        );
        assert_inputs("addr2line", &["--exe=app", "0"], &["app"]);
        assert_inputs("c++filt", &["_Z3foov", "not/a/file"], &[]);
        assert_inputs("strings", &["-s", "separator", "object.o"], &["object.o"]);
    }

    #[test]
    fn defaults_are_explicit_inputs() {
        assert_inputs("nm", &[], &["a.out"]);
        assert_inputs("size", &[], &["a.out"]);
        assert_inputs("addr2line", &["0"], &["a.out"]);
        assert!(effects("readelf", &command("readelf", &["-h"], None)).is_err());
    }

    #[test]
    fn stdin_modes_require_redirection_without_claiming_its_effects() {
        for kind in ["strings", "c++filt", "addr2line"] {
            assert!(effects(kind, &command(kind, &[], None)).is_err());
            let parsed = effects(kind, &command(kind, &[], Some("names"))).unwrap();
            let expected = if kind == "addr2line" {
                vec![PathBuf::from("a.out")]
            } else {
                Vec::new()
            };
            assert_eq!(parsed.inputs, expected);
            assert!(parsed.outputs.is_empty());
        }
    }

    #[test]
    fn debug_lookup_is_requested_only_by_accepted_lookup_modes() {
        for (kind, args, expected) in [
            ("objdump", vec!["-h", "object.o"], false),
            ("objdump", vec!["-d", "object.o"], true),
            ("objdump", vec!["-D", "object.o"], true),
            (
                "objdump",
                vec!["-d", "--dwarf=no-follow-links", "object.o"],
                false,
            ),
            ("addr2line", vec!["-e", "object.o", "0"], true),
            ("nm", vec!["-a", "object.o"], false),
        ] {
            assert_eq!(
                effects(kind, &command(kind, &args, None))
                    .unwrap()
                    .debug_lookup,
                expected
            );
        }
    }

    #[test]
    fn hidden_input_and_unknown_modes_fall_back() {
        for (kind, args) in [
            ("nm", vec!["--plugin=plugin.so", "object.o"]),
            ("nm", vec!["-l", "object.o"]),
            ("objdump", vec!["-S", "object.o"]),
            ("objdump", vec!["-l", "object.o"]),
            ("objdump", vec!["--source-comment=text", "object.o"]),
            ("objdump", vec!["--dwarf=follow-links", "object.o"]),
            ("objdump", vec!["-L", "object.o"]),
            ("readelf", vec!["-wK", "object.o"]),
            ("readelf", vec!["--process-links", "object.o"]),
            ("strings", vec!["--target=plugin", "object.o"]),
            ("addr2line", vec!["--plugin=plugin.so", "0"]),
            ("readobj", vec!["--unknown", "object.o"]),
            ("nm", vec!["--unknown", "object.o"]),
            ("size", vec!["--unknown", "object.o"]),
            ("c++filt", vec!["--unknown", "symbol"]),
        ] {
            assert!(
                effects(kind, &command(kind, &args, None)).is_err(),
                "{kind} {args:?}"
            );
        }
    }

    #[test]
    fn response_files_and_ambiguous_dash_inputs_fall_back() {
        for kind in [
            "nm",
            "objdump",
            "readelf",
            "size",
            "strings",
            "addr2line",
            "c++filt",
            "readobj",
        ] {
            assert!(effects(kind, &command(kind, &["@options"], None)).is_err());
            assert!(effects(kind, &command(kind, &["--", "@options"], None)).is_err());
        }
        assert!(effects("strings", &command("strings", &["-"], Some("data"))).is_err());
        assert_inputs("nm", &["--", "-object.o"], &["-object.o"]);
    }
}
