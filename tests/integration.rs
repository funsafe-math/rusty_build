use std::{fs, process::Command};

#[test]
fn cli_runs_native_file_lifecycle_without_creating_engine_state() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("source"), b"lifecycle").unwrap();
    fs::write(root.path().join("commands.txt"), "mkdir -p build/nested\ncp source build/nested/copied\nmv build/nested/copied build/nested/moved\ntouch build/nested/stamp\ncp build/nested/moved result\nrm -rf build\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rusty_make"))
        .current_dir(root.path())
        .args(["--commands", "commands.txt", "--verbose"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(root.path().join("result")).unwrap(), b"lifecycle");
    assert!(!root.path().join("build").exists());
    assert!(!root.path().join(".rusty_make").exists());
}

#[test]
fn removed_cli_options_are_not_accepted_or_executed() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("commands.txt"), "touch forbidden\n").unwrap();
    for flag in [
        "--dry-run",
        "--jobs",
        "--rebuild-all",
        "--dump-plan",
        "--compile-commands",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_rusty_make"))
            .current_dir(root.path())
            .args(["--commands", "commands.txt", flag])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(!root.path().join("forbidden").exists());
    }
}

#[cfg(unix)]
#[test]
fn cli_compiles_and_runs_a_real_generated_header_build() {
    if !Command::new("cc")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("template.h"), "#define ANSWER 42\n").unwrap();
    fs::write(root.path().join("main.c"), "#include \"generated.h\"\n#include <stdio.h>\nint main(void) { printf(\"%d\\n\", ANSWER); return 0; }\n").unwrap();
    fs::write(root.path().join("commands.txt"), "mkdir -p build\ncp template.h generated.h\ncc -I. -c main.c -o build/main.o\ncc build/main.o -o build/app\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rusty_make"))
        .current_dir(root.path())
        .args(["--commands", "commands.txt"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let app = Command::new(root.path().join("build/app"))
        .output()
        .unwrap();
    assert!(app.status.success());
    assert_eq!(app.stdout, b"42\n");
}
