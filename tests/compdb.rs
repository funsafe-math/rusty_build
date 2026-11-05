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

#[test]
fn windows_path_fields_are_lexical_and_keep_unc_absolute() {
    let root = tempfile::tempdir().unwrap();
    let graph = parse_plan("gcc -c main.c\n", root.path(), &Registry::default()).unwrap();
    let cases = [
        (
            r"\\?\C:\work\build",
            r"C:\work\main.c",
            r"C:\work\main.o",
            "C:/work/build",
            "C:/work/main.c",
            "C:/work/main.o",
        ),
        (
            r"\\?\C:/work/build",
            "C:/work/main.c",
            "C:/work/main.o",
            "C:/work/build",
            "C:/work/main.c",
            "C:/work/main.o",
        ),
        (
            r"\\?\UNC\server\share\build",
            r"\\?\UNC\server\share\main.c",
            r"\\server\share\main.o",
            "//server/share/build",
            "//server/share/main.c",
            "//server/share/main.o",
        ),
        (
            r"\\server\share\build",
            r"src\main.c",
            r"obj\main.o",
            "//server/share/build",
            "src/main.c",
            "obj/main.o",
        ),
    ];
    for (directory, file, output, expected_directory, expected_file, expected_output) in cases {
        let original = CompileCommand {
            directory: directory.into(),
            file: file.into(),
            output: Some(output.into()),
            arguments: vec![
                "gcc".into(),
                "-c".into(),
                file.into(),
                "-o".into(),
                output.into(),
            ],
        };
        let mut db = CompilationDatabase {
            commands: vec![original.clone()],
        };
        let path = root.path().join("db.json");
        db.save(&graph, &path).unwrap();
        let entries: Vec<CompileCommand> =
            serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(entries[0].directory, expected_directory);
        assert_eq!(entries[0].file, expected_file);
        assert_eq!(entries[0].output.as_deref(), Some(expected_output));
        assert_eq!(entries[0].arguments[2], expected_file);
        assert_eq!(entries[0].arguments[4], expected_output);
        assert_eq!(
            db.commands,
            [original],
            "serialization must format only its copy"
        );
    }
}

#[test]
fn windows_export_formats_recognized_operands_not_defines_or_unknown_arguments() {
    let root = tempfile::tempdir().unwrap();
    let mut graph = parse_plan(
        r#"gcc -I 'C:\sdk\include' '-IC:\sdk\local' -L 'C:/sdk/lib' '-DROOT=C:\literal\value' -D 'REGEX=\\[a-z]+\w' -c 'C:\src\main.c' -o 'C:\obj\main.o' --script 'C:\literal\script' '--regex=\\w+' "-fplugin=C:\literal\plugin"
"#,
        root.path(),
        &Registry::default(),
    ).unwrap();
    // No Windows filesystem is needed to exercise a Windows export directory.
    graph.cwd = std::path::PathBuf::from(r"\\?\C:\work\build");
    let before = serde_json::to_value(&graph).unwrap();
    let mut db = CompilationDatabase::from_graph(&graph).unwrap();
    assert_eq!(db.commands[0].directory, "C:/work/build");
    let path = root.path().join("db.json");
    db.save(&graph, &path).unwrap();
    let entries: Vec<CompileCommand> = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(
        entries[0].arguments,
        [
            "gcc",
            "-I",
            "C:/sdk/include",
            "-IC:/sdk/local",
            "-L",
            "C:/sdk/lib",
            r"-DROOT=C:\literal\value",
            "-D",
            r"REGEX=\\[a-z]+\w",
            "-c",
            "C:/src/main.c",
            "-o",
            "C:/obj/main.o",
            "--script",
            r"C:\literal\script",
            r"--regex=\\w+",
            r"-fplugin=C:\literal\plugin",
        ]
    );
    assert_eq!(entries[0].file, "C:/src/main.c");
    assert_eq!(entries[0].output.as_deref(), Some("C:/obj/main.o"));
    assert_eq!(serde_json::to_value(&graph).unwrap(), before);
}

#[test]
fn transform_paths_are_formatted_after_hook_without_changing_hook_values() {
    let root = tempfile::tempdir().unwrap();
    let graph = parse_plan(
        "gcc -c main.c -o main.o\n",
        root.path(),
        &Registry::default(),
    )
    .unwrap();
    let mut db = CompilationDatabase::from_graph(&graph).unwrap();
    let path = root.path().join("db.json");
    db.save_with_transform(&graph, &path, |_, db| {
        db.commands[0].directory = r"\\?\UNC\server\share\build".into();
        db.commands[0].file = r"\\?\C:\relocated\main.c".into();
        db.commands[0].output = Some(r"obj\main.o".into());
        db.commands[0].arguments = [
            r"C:\tools\gcc.exe",
            "-isystem",
            r"C:\sdk\include",
            r"--sysroot=C:\sdk",
            "-MT",
            r"C:\literal\target",
            "-c",
            r"\\?\C:\relocated\main.c",
            "-o",
            r"obj\main.o",
        ]
        .map(str::to_owned)
        .to_vec();
        Ok(())
    })
    .unwrap();
    let entries: Vec<CompileCommand> = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(entries[0].directory, "//server/share/build");
    assert_eq!(entries[0].file, "C:/relocated/main.c");
    assert_eq!(entries[0].output.as_deref(), Some("obj/main.o"));
    assert_eq!(
        entries[0].arguments,
        [
            "C:/tools/gcc.exe",
            "-isystem",
            "C:/sdk/include",
            "--sysroot=C:/sdk",
            "-MT",
            r"C:\literal\target",
            "-c",
            "C:/relocated/main.c",
            "-o",
            "obj/main.o",
        ]
    );
    assert_eq!(db.commands[0].directory, r"\\?\UNC\server\share\build");
    assert_eq!(db.commands[0].file, r"\\?\C:\relocated\main.c");
}

#[test]
fn posix_backslash_filenames_and_spelling_are_not_rewritten() {
    let root = tempfile::tempdir().unwrap();
    let graph = parse_plan("gcc -c main.c\n", root.path(), &Registry::default()).unwrap();
    let original = CompileCommand {
        directory: "/posix/build/../build".into(),
        file: r"src\literal.c".into(),
        output: Some(r"/posix/obj\literal.o".into()),
        arguments: [
            "gcc",
            "-I",
            r"include\literal",
            r"-I/posix/include\literal",
            "-c",
            r"src\literal.c",
            "-o",
            r"/posix/obj\literal.o",
            "-D",
            r"C:\literal\define",
            "--unknown",
            r"C:\literal\script",
        ]
        .map(str::to_owned)
        .to_vec(),
    };
    let mut db = CompilationDatabase {
        commands: vec![original.clone()],
    };
    let path = root.path().join("db.json");
    db.save(&graph, &path).unwrap();
    let entries: Vec<CompileCommand> = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(entries, [original]);
}

#[test]
fn cli_exports_windows_source_spellings_without_executing_compiler() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("commands.txt"),
        "gcc -c 'C:/src/one.c' -o 'C:/obj/one.o'\ngcc -c 'C:\\src\\two.c' -o 'C:\\obj\\two.o'\n",
    )
    .unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_rusty_make"))
        .current_dir(root.path())
        .args([
            "--commands",
            "commands.txt",
            "--dry-run",
            "--compile-commands",
            "compile_commands.json",
            "--dump-plan",
            "plan.json",
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
        serde_json::from_slice(&fs::read(root.path().join("compile_commands.json")).unwrap())
            .unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].file, "C:/src/one.c");
    assert_eq!(entries[1].file, "C:/src/two.c");
    assert_eq!(entries[1].output.as_deref(), Some("C:/obj/two.o"));
    assert_eq!(
        entries[1].arguments,
        ["gcc", "-c", "C:/src/two.c", "-o", "C:/obj/two.o"]
    );
    for entry in &entries {
        assert!(!entry.directory.starts_with(r"\\?\"));
        assert!(!entry.directory.contains('\\'));
    }
    let plan: serde_json::Value =
        serde_json::from_slice(&fs::read(root.path().join("plan.json")).unwrap()).unwrap();
    assert_eq!(
        plan["jobs"][1]["spec"]["parsed"]["argv"][2],
        r"C:\src\two.c"
    );
}
