use rusty_make::{
    compdb::{CompilationDatabase, CompileCommand},
    parse_plan,
    registry::Registry,
};
use std::fs;
#[test]
fn exports_argv_and_graph_aware_transform_before_save() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    let graph=parse_plan("mkdir -p build\ngcc -I include -Iother -DDEBUG -O2 -c main.c -o build/main.o\ngcc build/main.o -o build/app\n",p,&Registry::default()).unwrap();
    let mut database = CompilationDatabase::from_graph(&graph).unwrap();
    let path = p.join("compile_commands.json");
    database
        .save_with_transform(&graph, &path, |graph, db| {
            let compile = graph
                .jobs
                .iter()
                .position(|j| j.spec.kind == "compile")
                .unwrap();
            if graph.edges.iter().any(|e| e.to == compile) {
                db.commands[0]
                    .arguments
                    .push("-DGRAPH_HAS_PREREQUISITE".into());
            }
            db.commands[0].directory = "/relocated/build".into();
            Ok(())
        })
        .unwrap();
    let entries: Vec<CompileCommand> = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].file, "main.c");
    assert_eq!(entries[0].output.as_deref(), Some("build/main.o"));
    assert_eq!(entries[0].directory, "/relocated/build");
    assert_eq!(
        entries[0].arguments,
        vec![
            "gcc",
            "-I",
            "include",
            "-Iother",
            "-DDEBUG",
            "-O2",
            "-c",
            "main.c",
            "-o",
            "build/main.o",
            "-DGRAPH_HAS_PREREQUISITE"
        ]
    );
}
#[test]
fn transform_failure_never_overwrites_existing_database() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    let graph = parse_plan("cc -c main.c -o main.o\n", p, &Registry::default()).unwrap();
    let mut db = CompilationDatabase::from_graph(&graph).unwrap();
    let path = p.join("db.json");
    fs::write(&path, "existing").unwrap();
    let error = db.save_with_transform(&graph, &path, |_, _| anyhow::bail!("invalid relocation"));
    assert!(error.is_err());
    assert_eq!(fs::read_to_string(path).unwrap(), "existing");
}
#[test]
fn cli_exports_database_without_running_compiler() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("commands.txt"), "gcc -c absent.c -o absent.o\n").unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_rusty_make"))
        .current_dir(p)
        .args([
            "--commands",
            "commands.txt",
            "--dry-run",
            "--compile-commands",
            "compile_commands.json",
            "--dump-plan",
            "plan.json",
            "--dump-graph",
            "graph.dot",
            "--rebuild-all",
            "--no-optimize",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let entries: Vec<CompileCommand> =
        serde_json::from_slice(&fs::read(p.join("compile_commands.json")).unwrap()).unwrap();
    assert_eq!(
        entries[0].arguments,
        vec!["gcc", "-c", "absent.c", "-o", "absent.o"]
    );
    assert!(!p.join("absent.o").exists());
    let plan: serde_json::Value =
        serde_json::from_slice(&fs::read(p.join("plan.json")).unwrap()).unwrap();
    assert_eq!(plan["jobs"][0]["spec"]["kind"], "compile");
    assert!(
        fs::read_to_string(p.join("graph.dot"))
            .unwrap()
            .contains("absent.c")
    );
}

#[test]
fn compile_and_link_exports_each_source_but_not_pure_link_inputs() {
    let root = tempfile::tempdir().unwrap();
    let graph = parse_plan(
        "gcc one.c two.c library.o -o app\n",
        root.path(),
        &Registry::default(),
    )
    .unwrap();
    let db = CompilationDatabase::from_graph(&graph).unwrap();
    assert_eq!(
        db.commands
            .iter()
            .map(|entry| entry.file.as_str())
            .collect::<Vec<_>>(),
        vec!["one.c", "two.c"]
    );
    for entry in db.commands {
        assert_eq!(
            entry.arguments,
            vec!["gcc", "one.c", "two.c", "library.o", "-o", "app"]
        );
        assert_eq!(entry.output.as_deref(), Some("app"));
    }
}
