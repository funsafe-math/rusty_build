use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};
fn copy_tree(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let target = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}
fn hashes(root: &Path, dir: &Path, result: &mut BTreeMap<PathBuf, String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() == ".rusty_make" {
            continue;
        }
        if entry.file_type().unwrap().is_dir() {
            hashes(root, &entry.path(), result);
        } else {
            result.insert(
                entry.path().strip_prefix(root).unwrap().to_owned(),
                blake3::hash(&fs::read(entry.path()).unwrap())
                    .to_hex()
                    .to_string(),
            );
        }
    }
}
#[test]
fn optimized_cli_matches_sequential_shell_reference() {
    for tool in ["gcc", "python3", "sed", "bash"] {
        if Command::new(tool).arg("--version").output().is_err() {
            eprintln!("skipping unavailable integration tool: {tool}");
            return;
        }
    }
    let root = tempfile::tempdir().unwrap();
    let reference = root.path().join("reference");
    let optimized = root.path().join("optimized");
    let unoptimized = root.path().join("unoptimized");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/demo");
    for path in [&reference, &optimized, &unoptimized] {
        copy_tree(&fixture, path);
    }
    let log = fs::read_to_string(fixture.join("commands.txt")).unwrap();
    for command in rusty_make::input::read_log(&log).unwrap().commands {
        let status = Command::new("bash")
            .args(["--noprofile", "--norc", "-c", &command.text])
            .current_dir(&reference)
            .status()
            .unwrap();
        assert!(status.success(), "{}", command.text);
    }
    for (path, no_optimize) in [(&optimized, false), (&unoptimized, true)] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rusty_make"));
        command.current_dir(path).args([
            "--commands",
            "commands.txt",
            "-j4",
            "--rebuild-all",
            "--explain",
            "--python-script",
            "generate=tools/generate.py",
            "--python-script",
            "transform=tools/transform.py",
        ]);
        if no_optimize {
            command.arg("--no-optimize");
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("unknown: printf marker"));
        assert!(String::from_utf8_lossy(&output.stderr).contains("generate"));
        assert!(String::from_utf8_lossy(&output.stderr).contains("transform"));
        let app = Command::new(path.join("build/app")).output().unwrap();
        assert!(app.status.success());
        assert_eq!(app.stdout, b"12\n");
    }
    let mut expected = BTreeMap::new();
    hashes(&reference, &reference, &mut expected);
    for path in [&optimized, &unoptimized] {
        let mut actual = BTreeMap::new();
        hashes(path, path, &mut actual);
        assert_eq!(expected, actual);
    }
}
#[test]
fn cli_make_wrapper_executes_only_successful_plan() {
    if Command::new("make").arg("--version").output().is_err() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("src"), "make planned\n").unwrap();
    fs::write(
        p.join("Makefile"),
        "$(file >generated,expansion output)\nall: out\nout: src\n\tcp src out\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rusty_make"))
        .current_dir(p)
        .args(["-j2", "--explain"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_to_string(p.join("out")).unwrap(), "make planned\n");
    assert_eq!(
        fs::read_to_string(p.join("generated")).unwrap(),
        "expansion output\n"
    );
    fs::remove_file(p.join("out")).unwrap();
    fs::write(
        p.join("Makefile"),
        "all: first missing\nfirst:\n\tcp src out\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rusty_make"))
        .current_dir(p)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!p.join("out").exists());
}

#[test]
fn make_variable_precedence_respects_mixed_argument_order() {
    if Command::new("make").arg("--version").output().is_err() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("first"), "first").unwrap();
    fs::write(p.join("second"), "second").unwrap();
    fs::write(p.join("Makefile"), "all:\n\tcp $(VALUE) output\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rusty_make"))
        .current_dir(p)
        .args(["VALUE=first", "--make-arg=VALUE=second", "-j2"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_to_string(p.join("output")).unwrap(), "second");
}
