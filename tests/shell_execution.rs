use rusty_make::{
    parse_plan,
    registry::Registry,
    scheduler::{RunOptions, run},
};
use std::fs;
#[test]
fn numeric_stdin_descriptor_never_becomes_an_extra_copy_operand() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("a"), "payload").unwrap();
    fs::write(p.join("input"), "stdin").unwrap();
    let graph = parse_plan("cp a b 0<input\n0<input cp a c\n", p, &Registry::default()).unwrap();
    run(&graph, &RunOptions::default()).unwrap();
    assert_eq!(fs::read_to_string(p.join("b")).unwrap(), "payload");
    assert_eq!(fs::read_to_string(p.join("c")).unwrap(), "payload");
}

#[cfg(unix)]
#[test]
fn unknown_created_symlink_ancestors_cannot_invalidate_native_hazard_model() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("source"), "new").unwrap();
    fs::create_dir(p.join("real")).unwrap();
    let graph = parse_plan(
        "ln -s real alias\ncp source alias/output\n",
        p,
        &Registry::default(),
    )
    .unwrap();
    assert!(run(&graph, &RunOptions::default()).is_err());
    assert!(!p.join("real/output").exists());
}

#[test]
fn literal_space_paths_and_sed_redirections_execute_without_flattening() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("file with spaces"), "foo foo\n").unwrap();
    let graph = parse_plan(
        "cp \"file with spaces\" copied\nsed -e 's/foo/bar/g' copied > transformed\nsed 's/bar/baz/g' transformed > output\nsed -i 's/baz/qux/g' output\ncat < output > redirected\nprintf 'tail\\n' >> redirected\n",
        p, &Registry::default(),
    ).unwrap();
    run(
        &graph,
        &RunOptions {
            jobs: 4,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(p.join("transformed")).unwrap(),
        "bar bar\n"
    );
    assert_eq!(fs::read_to_string(p.join("output")).unwrap(), "qux qux\n");
    assert_eq!(
        fs::read_to_string(p.join("redirected")).unwrap(),
        "qux qux\ntail\n"
    );
}

#[test]
fn persisted_redirected_jobs_keep_shell_file_effects_and_execute() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("seed"), "foo foo\n").unwrap();
    let registry = Registry::default();
    let graph = parse_plan(
        "cp seed copied > copy.log\nsed -e 's/foo/bar/g' copied > transformed\n",
        root.path(),
        &registry,
    )
    .unwrap();
    let bytes = serde_json::to_vec(&graph).unwrap();
    let restored = rusty_make::graph::BuildGraph::from_json(&bytes, &registry).unwrap();
    run(&restored, &RunOptions::default()).unwrap();
    assert_eq!(fs::read(root.path().join("copied")).unwrap(), b"foo foo\n");
    assert_eq!(
        fs::read(root.path().join("transformed")).unwrap(),
        b"bar bar\n"
    );
    assert_eq!(fs::read(root.path().join("copy.log")).unwrap(), b"");
}

#[test]
fn persisted_redirected_compiler_executes_its_original_shell_command() {
    use std::process::Command;
    if !Command::new("cc")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        eprintln!("skipping compiler restoration: cc is unavailable");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("main.c"),
        "#include <stdio.h>\nint main(void) { puts(\"restored compiler\"); return 0; }\n",
    )
    .unwrap();
    let registry = Registry::default();
    let graph = parse_plan("cc main.c -o app > compiler.log\n", root.path(), &registry).unwrap();
    let bytes = serde_json::to_vec(&graph).unwrap();
    let restored = rusty_make::graph::BuildGraph::from_json(&bytes, &registry).unwrap();
    run(&restored, &RunOptions::default()).unwrap();
    let output = Command::new(root.path().join("app")).output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"restored compiler\n");
    assert!(output.stderr.is_empty());
    assert_eq!(fs::read(root.path().join("compiler.log")).unwrap(), b"");
}

#[test]
fn persisted_null_streams_preserve_file_effects_and_need_no_shell() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("seed"), b"null streams are not artifacts").unwrap();
    let text = "cp seed copied > /dev/null 2> /dev/null\ncp copied final 2> /dev/null\n";
    fs::write(p.join("commands.txt"), text).unwrap();
    let registry = Registry::default();
    let graph = parse_plan(text, p, &registry).unwrap();
    for record in &graph.jobs {
        assert!(
            !record
                .spec
                .inputs
                .iter()
                .chain(&record.spec.outputs)
                .any(|path| rusty_make::job::is_null_device(path))
        );
    }
    let restored =
        rusty_make::graph::BuildGraph::from_json(&serde_json::to_vec(&graph).unwrap(), &registry)
            .unwrap();
    run(&restored, &RunOptions::default()).unwrap();
    assert_eq!(
        fs::read(p.join("final")).unwrap(),
        b"null streams are not artifacts"
    );
    fs::remove_file(p.join("copied")).unwrap();
    fs::remove_file(p.join("final")).unwrap();
    let empty_path = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_rusty_make"))
        .current_dir(p)
        .env("PATH", empty_path.path())
        .args([
            "--commands",
            "commands.txt",
            "--no-optimize",
            "--rebuild-all",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read(p.join("final")).unwrap(),
        b"null streams are not artifacts"
    );
}

#[test]
fn ordinary_noop_fallback_retains_failure_masking_after_restore() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("seed"), b"continue after masked failure").unwrap();
    let registry = Registry::default();
    let graph = parse_plan(
        "cp missing absent 2> /dev/null || cd .\ncp seed final\n",
        p,
        &registry,
    )
    .unwrap();
    let restored =
        rusty_make::graph::BuildGraph::from_json(&serde_json::to_vec(&graph).unwrap(), &registry)
            .unwrap();
    run(&restored, &RunOptions::default()).unwrap();
    assert!(!p.join("absent").exists());
    assert_eq!(
        fs::read(p.join("final")).unwrap(),
        b"continue after masked failure"
    );
}

#[test]
fn execution_cwd_supports_forward_slash_operands_and_legacy_plan_roots() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().join("working tree é");
    fs::create_dir(&cwd).unwrap();
    fs::write(cwd.join("input.txt"), b"cwd payload").unwrap();
    #[cfg(windows)]
    let supplied_cwd = std::path::PathBuf::from(cwd.to_string_lossy().replace('\\', "/"));
    #[cfg(not(windows))]
    let supplied_cwd = cwd.clone();
    let registry = Registry::default();
    let graph = parse_plan(
        "mkdir -p 'build tree/objects'\ncp input.txt 'build tree/objects/output.txt'\n\
         touch 'build tree/objects/stamp'\n",
        &supplied_cwd,
        &registry,
    )
    .unwrap();
    let options = RunOptions {
        rebuild_all: true,
        ..Default::default()
    };
    run(&graph, &options).unwrap();
    assert_eq!(
        fs::read(cwd.join("build tree/objects/output.txt")).unwrap(),
        b"cwd payload"
    );
    assert!(cwd.join("build tree/objects/stamp").is_file());

    // Simulate an older plan's canonicalized cwd, including \\?\ on Windows.
    let mut stored = serde_json::to_value(&graph).unwrap();
    stored["cwd"] = serde_json::to_value(cwd.canonicalize().unwrap()).unwrap();
    let restored =
        rusty_make::graph::BuildGraph::from_json(&serde_json::to_vec(&stored).unwrap(), &registry)
            .unwrap();
    fs::remove_file(cwd.join("build tree/objects/output.txt")).unwrap();
    fs::remove_file(cwd.join("build tree/objects/stamp")).unwrap();
    run(&restored, &options).unwrap();
    assert_eq!(
        fs::read(cwd.join("build tree/objects/output.txt")).unwrap(),
        b"cwd payload"
    );
}

#[test]
fn mkdir_uses_relative_build_cwd_and_preserves_existing_files() {
    let process_cwd = std::env::current_dir().unwrap();
    let root = tempfile::tempdir_in(&process_cwd).unwrap();
    fs::write(root.path().join("source"), b"relative cwd payload").unwrap();
    let relative_cwd = root.path().strip_prefix(&process_cwd).unwrap();
    let registry = Registry::default();
    let graph = parse_plan(
        "mkdir -p 'tree/leaf'\ncp source 'tree/leaf/result'\nmkdir -p 'tree/leaf'\n",
        relative_cwd,
        &registry,
    )
    .unwrap();
    let options = RunOptions {
        rebuild_all: true,
        ..Default::default()
    };
    run(&graph, &options).unwrap();
    assert_eq!(
        fs::read(root.path().join("tree/leaf/result")).unwrap(),
        b"relative cwd payload"
    );
    fs::write(root.path().join("blocker"), b"keep file").unwrap();
    let blocked = parse_plan("mkdir -p blocker/child\n", relative_cwd, &registry).unwrap();
    assert!(run(&blocked, &options).is_err());
    assert_eq!(fs::read(root.path().join("blocker")).unwrap(), b"keep file");
}

#[cfg(windows)]
#[test]
fn mkdir_accepts_complete_windows_drive_and_verbatim_paths() {
    let root = tempfile::tempdir().unwrap();
    let ordinary = root.path().join("absolute tree/leaf");
    let verbatim = root
        .path()
        .canonicalize()
        .unwrap()
        .join("verbatim tree/leaf");
    let forward = ordinary.to_string_lossy().replace('\\', "/");
    let text = format!(
        "mkdir -p {} {}\ncp input.txt {}\n",
        shell_words::quote(&forward),
        shell_words::quote(verbatim.to_str().unwrap()),
        shell_words::quote(&format!("{forward}/output.txt")),
    );
    fs::write(root.path().join("input.txt"), b"complete Windows paths").unwrap();
    let graph = parse_plan(&text, root.path(), &Registry::default()).unwrap();
    run(&graph, &RunOptions::default()).unwrap();
    assert!(ordinary.is_dir());
    assert!(verbatim.is_dir());
    assert_eq!(
        fs::read(ordinary.join("output.txt")).unwrap(),
        b"complete Windows paths"
    );
}
