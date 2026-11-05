use rusty_make::{
    compdb::CompilationDatabase,
    parse_plan,
    registry::Registry,
    scheduler::{RunOptions, run},
};
use std::{fs, process::Command};

#[test]
fn include_and_library_paths_survive_unknown_flags_and_database_export() {
    let root = tempfile::tempdir().unwrap();
    let text = "gcc -Wall -Wextra -std=c11 -I include -I'other include' -L lib -L'other lib' -c main.c -o main.o\n";
    let graph = parse_plan(text, root.path(), &Registry::default()).unwrap();
    let job = &graph.jobs[0];
    assert_eq!(
        job.spec.parsed["includes"],
        serde_json::json!(["include", "other include"])
    );
    assert_eq!(
        job.spec.parsed["library_paths"],
        serde_json::json!(["lib", "other lib"])
    );
    assert_eq!(
        job.spec.parsed["unparsed_args"],
        serde_json::json!(["-Wall", "-Wextra", "-std=c11"])
    );
    assert!(job.spec.unknown_effects && !job.spec.cacheable);
    let database = CompilationDatabase::from_graph(&graph).unwrap();
    assert_eq!(database.commands[0].file, "main.c");
    assert_eq!(
        database.commands[0].arguments,
        vec![
            "gcc",
            "-Wall",
            "-Wextra",
            "-std=c11",
            "-I",
            "include",
            "-Iother include",
            "-L",
            "lib",
            "-Lother lib",
            "-c",
            "main.c",
            "-o",
            "main.o"
        ]
    );
}

#[test]
fn unknown_option_operands_and_response_files_are_not_guessed_as_sources() {
    let root = tempfile::tempdir().unwrap();
    let graph = parse_plan(
        "gcc -include generated.c -c main.c -o main.o\ngcc @args.rsp\n",
        root.path(),
        &Registry::default(),
    )
    .unwrap();
    assert_eq!(
        graph.jobs[0].spec.parsed["sources"],
        serde_json::json!(["main.c"])
    );
    assert_eq!(
        graph.jobs[0].spec.parsed["unparsed_args"],
        serde_json::json!(["-include", "generated.c"])
    );
    assert_eq!(graph.jobs[1].spec.parsed["sources"], serde_json::json!([]));
    assert!(graph.jobs[1].spec.outputs.is_empty());
    let db = CompilationDatabase::from_graph(&graph).unwrap();
    assert_eq!(
        db.commands
            .iter()
            .map(|c| c.file.as_str())
            .collect::<Vec<_>>(),
        vec!["main.c"]
    );
}

#[test]
fn extended_optimization_and_debug_flags_are_forwarded_not_rejected() {
    let root = tempfile::tempdir().unwrap();
    let graph = parse_plan(
        "gcc -Ofuture -ggdb -c main.c -o main.o\n",
        root.path(),
        &Registry::default(),
    )
    .unwrap();
    assert_eq!(
        graph.jobs[0].spec.parsed["unparsed_args"],
        serde_json::json!(["-Ofuture", "-ggdb"])
    );
    assert_eq!(
        graph.jobs[0].spec.parsed["sources"],
        serde_json::json!(["main.c"])
    );
}

#[test]
fn real_compiler_uses_include_and_library_paths_with_passthrough_flags() {
    for tool in ["cc", "ar"] {
        if Command::new(tool).arg("--version").output().is_err() {
            eprintln!("skipping unavailable compiler fixture tool: {tool}");
            return;
        }
    }
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    for dir in ["include one", "include2", "library one"] {
        fs::create_dir(p.join(dir)).unwrap();
    }
    fs::write(p.join("include one/value.h"), "int value(void);\n").unwrap();
    fs::write(p.join("include2/offset.h"), "#define OFFSET 5\n").unwrap();
    fs::write(p.join("value.c"), "int value(void) { return 7; }\n").unwrap();
    fs::write(p.join("main.c"), "#include <value.h>\n#include <offset.h>\n#include <stdio.h>\nint main(void) { printf(\"%d\\n\", value() + OFFSET); return value() + OFFSET == 12 ? 0 : 1; }\n").unwrap();
    let text = "mkdir -p build\ncc -c value.c -o build/value.o\nar rcs 'library one/libvalue.a' build/value.o\ncc -Wall -Wextra -std=c11 -I 'include one' -Iinclude2 -L 'library one' -Llibrary2 -c main.c -o build/main.o\ncc build/main.o -L 'library one' -Llibrary2 -lvalue -o build/app\n";
    let graph = parse_plan(text, p, &Registry::default()).unwrap();
    run(
        &graph,
        &RunOptions {
            jobs: 4,
            ..Default::default()
        },
    )
    .unwrap();
    let output = Command::new(p.join("build/app")).output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"12\n");
}

#[test]
fn compiler_itself_rejects_invalid_forwarded_flags_and_stops_dependents() {
    if Command::new("cc").arg("--version").output().is_err() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("main.c"), "int value(void) { return 1; }\n").unwrap();
    let graph = parse_plan(
        "cc --rusty-make-invalid-flag -c main.c -o main.o\ncp main.c should-not-exist\n",
        p,
        &Registry::default(),
    )
    .unwrap();
    assert!(run(&graph, &RunOptions::default()).is_err());
    assert!(!p.join("main.o").exists());
    assert!(!p.join("should-not-exist").exists());
}

#[test]
fn known_options_still_require_values_and_preserve_hyphen_path_values() {
    let root = tempfile::tempdir().unwrap();
    let registry = Registry::default();
    for command in ["cc -I\n", "cc -L\n", "cc -o\n", "cc -c a.c b.c -o one.o\n"] {
        assert!(
            parse_plan(command, root.path(), &registry).is_err(),
            "{command}"
        );
    }
    let graph = parse_plan(
        "cc -I -headers -L -libraries -c main.c -o main.o\n",
        root.path(),
        &registry,
    )
    .unwrap();
    assert_eq!(
        graph.jobs[0].spec.parsed["includes"],
        serde_json::json!(["-headers"])
    );
    assert_eq!(
        graph.jobs[0].spec.parsed["library_paths"],
        serde_json::json!(["-libraries"])
    );
}
