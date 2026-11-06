use rusty_make::{
    commands::compiler::Compiler, graph::create_graph, job::FileAccess, parse, registry::Registry,
    scheduler::execute, shell::ShellHints,
};
use std::{any::Any, fs, process::Command};

fn compiler(args: &[&str]) -> Compiler {
    let argv = std::iter::once("cc")
        .chain(args.iter().copied())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    Compiler::parse(&argv, &ShellHints::default()).unwrap()
}

#[test]
fn original_argv_and_string_metadata_survive_unknown_flags() {
    let argv = [
        "/tool chain/custom cc",
        "-Wall",
        "-Wextra",
        "-std=c11",
        "-I",
        "include",
        "-Iother include",
        "-L",
        "lib",
        "-Lother lib",
        "-DANSWER=42",
        "-O2",
        "-g",
        "-c",
        "./main.c",
        "-o",
        "obj/../main.o",
    ]
    .map(str::to_owned);
    let job = Compiler::parse(&argv, &ShellHints::default()).unwrap();
    assert_eq!(job.argv, argv);
    assert_eq!(job.sources, ["./main.c"]);
    assert_eq!(job.inputs, ["./main.c"]);
    assert_eq!(job.outputs, ["obj/../main.o"]);
    assert_eq!(job.include_dirs, ["include", "other include"]);
    assert_eq!(job.library_paths, ["lib", "other lib"]);
    assert_eq!(job.defines, ["ANSWER=42"]);
    assert_eq!(job.optimization, ["2"]);
    assert_eq!(job.debug, ["2"]);
    assert_eq!(job.unparsed_args, ["-Wall", "-Wextra", "-std=c11"]);
    assert!(!job.metadata_complete);
    assert_eq!(
        job.data.accesses,
        [
            FileAccess::read("./main.c"),
            FileAccess::write("obj/../main.o")
        ]
    );
    assert!(job.data.unknown_effects);
}

#[test]
fn unknown_option_arity_and_response_file_contents_are_never_guessed() {
    let job = compiler(&[
        "-include",
        "generated.c",
        "extra.c",
        "-c",
        "main.c",
        "-o",
        "main.o",
    ]);
    assert_eq!(job.sources, ["main.c"]);
    assert_eq!(job.unparsed_args, ["-include", "generated.c", "extra.c"]);
    let unknown = compiler(&["-future-option", "maybe.c"]);
    assert!(unknown.sources.is_empty());
    assert!(unknown.outputs.is_empty());
    let response = compiler(&["@args.rsp"]);
    assert!(response.inputs.is_empty());
    assert!(response.outputs.is_empty());
    assert!(response.data.unknown_effects);
    let explicit = compiler(&["-unknown", "value.c", "-c", "--", "-literal.c"]);
    assert_eq!(explicit.sources, ["-literal.c"]);
    assert_eq!(explicit.unparsed_args, ["-unknown", "value.c"]);
}

#[test]
fn include_categories_and_default_outputs_are_understood_but_effects_stay_conservative() {
    let job = compiler(&[
        "-isystemsystem",
        "-Ilocal",
        "-iquote",
        "quoted",
        "-idirafterafter",
        "-c",
        "src/a.c",
        "src/b.cpp",
    ]);
    assert_eq!(job.include_dirs, ["local", "system", "quoted", "after"]);
    assert_eq!(job.outputs, ["a.o", "b.o"]);
    assert!(job.metadata_complete);
    assert!(
        job.data.unknown_effects,
        "implicit includes and compiler/toolchain effects remain unaccounted"
    );
    let header = compiler(&["-c", "include/api.h"]);
    assert_eq!(header.outputs, ["include/api.h.gch"]);
    let link = compiler(&["a.o", "b.c"]);
    assert_eq!(link.sources, ["b.c"]);
    assert_eq!(
        link.outputs,
        [if cfg!(windows) { "a.exe" } else { "a.out" }]
    );
    let extensions = compiler(&["-Ofuture", "-ggdb", "-c", "main.c", "-o", "main.o"]);
    assert_eq!(extensions.unparsed_args, ["-Ofuture", "-ggdb"]);
    assert_eq!(extensions.sources, ["main.c"]);
}

#[test]
fn known_options_require_values_and_keep_hyphen_path_values() {
    for args in [
        vec![],
        vec!["-I"],
        vec!["-L"],
        vec!["-D"],
        vec!["-o"],
        vec!["-c", "a.c", "b.c", "-o", "one.o"],
    ] {
        let argv = std::iter::once("cc")
            .chain(args)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert!(
            Compiler::parse(&argv, &ShellHints::default()).is_err(),
            "{argv:?}"
        );
    }
    let job = compiler(&[
        "-I",
        "-headers",
        "-L",
        "-libraries",
        "-c",
        "main.c",
        "-o",
        "main.o",
    ]);
    assert_eq!(job.include_dirs, ["-headers"]);
    assert_eq!(job.library_paths, ["-libraries"]);
}

#[test]
fn compiler_preflight_checks_declared_inputs_without_creating_outputs() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("missing.c");
    let output = root.path().join("main.o");
    let text = format!("cc -c '{}' -o '{}'\n", source.display(), output.display());
    let dag = create_graph(parse(&text, &Registry::default()).unwrap()).unwrap();
    assert!(dag.check_inputs().is_err());
    assert!(!output.exists());
    fs::write(&source, "int value(void) { return 42; }\n").unwrap();
    dag.check_inputs().unwrap();
    let ignored = create_graph(
        parse(
            &format!(
                "cc -c '{}' -o '{}' || true\n",
                root.path().join("ignored.c").display(),
                output.display()
            ),
            &Registry::default(),
        )
        .unwrap(),
    )
    .unwrap();
    ignored.check_inputs().unwrap();
}

#[test]
fn real_compiler_uses_original_include_and_library_arguments() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    for directory in ["include one", "include2", "library one", "build"] {
        fs::create_dir(p.join(directory)).unwrap();
    }
    fs::write(p.join("include one/value.h"), "int value(void);\n").unwrap();
    fs::write(p.join("include2/offset.h"), "#define OFFSET 5\n").unwrap();
    fs::write(p.join("value.c"), "int value(void) { return 7; }\n").unwrap();
    fs::write(p.join("main.c"), "#include <value.h>\n#include <offset.h>\n#include <stdio.h>\nint main(void) { printf(\"%d\\n\", value() + OFFSET); return value() + OFFSET == 12 ? 0 : 1; }\n").unwrap();
    let text = format!(
        "cc -c '{}/value.c' -o '{}/build/value.o'\nar rcs '{}/library one/libvalue.a' '{}/build/value.o'\ncc -Wall -Wextra -std=c11 -I '{}/include one' -I'{}/include2' -L '{}/library one' -c '{}/main.c' -o '{}/build/main.o'\ncc '{}/build/main.o' -L '{}/library one' -lvalue -o '{}/build/app'\n",
        p.display(),
        p.display(),
        p.display(),
        p.display(),
        p.display(),
        p.display(),
        p.display(),
        p.display(),
        p.display(),
        p.display(),
        p.display(),
        p.display(),
    );
    let jobs = parse(&text, &Registry::default()).unwrap();
    let compile = (jobs[2].as_ref() as &dyn Any)
        .downcast_ref::<Compiler>()
        .unwrap();
    assert!(compile.argv.iter().any(|arg| arg == "-Wall"));
    let dag = create_graph(jobs).unwrap();
    dag.check_inputs().unwrap();
    execute(&dag, false).unwrap();
    let output = Command::new(p.join("build/app")).output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"12\n");
}

#[test]
fn invalid_forwarded_compiler_flag_stops_dependent_jobs() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("main.c"), "int value(void) { return 1; }\n").unwrap();
    let text = format!(
        "cc --rusty-make-invalid-flag -c '{}/main.c' -o '{}/main.o'\ncp '{}/main.c' '{}/should-not-exist'\n",
        p.display(),
        p.display(),
        p.display(),
        p.display()
    );
    let dag = create_graph(parse(&text, &Registry::default()).unwrap()).unwrap();
    dag.check_inputs().unwrap();
    assert!(execute(&dag, false).is_err());
    assert!(!p.join("main.o").exists());
    assert!(!p.join("should-not-exist").exists());
}
