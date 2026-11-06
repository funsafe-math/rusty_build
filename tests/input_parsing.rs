use rusty_make::{input::read_log, parse, registry::Registry};
use std::{fs, process::Command};

#[test]
fn literal_arguments_and_redirect_targets_survive_tokenization() {
    let lines =
        read_log("sed 's/foo/>/g' 'input file'>'output file' 2>/dev/null || cd .\n").unwrap();
    assert_eq!(lines[0].argv, ["sed", "s/foo/>/g", "input file"]);
    assert_eq!(lines[0].shell.stdout.as_deref(), Some("output file"));
    assert_eq!(lines[0].shell.stderr.as_deref(), Some("/dev/null"));
    assert!(lines[0].shell.ignore_failure);
    let line = read_log("gcc '-DPATTERN=\\d+' -c 'src/a file.c' -o 'obj/a file.o'\n").unwrap();
    assert_eq!(
        line[0].argv,
        [
            "gcc",
            "-DPATTERN=\\d+",
            "-c",
            "src/a file.c",
            "-o",
            "obj/a file.o"
        ]
    );
}

#[test]
fn crlf_continuations_keep_physical_line_identity() {
    let log = read_log("# heading\r\ncp se\\\r\ned result\r\ntouch next\r\n").unwrap();
    assert_eq!(log[0].argv, ["cp", "seed", "result"]);
    assert_eq!(log[0].line, 2);
    assert_eq!(log[1].line, 4);
    let log = read_log("tool \"ab\\\r\ncd\" 'literal\\r\\n'\r\n").unwrap();
    assert_eq!(log[0].argv, ["tool", "abcd", "literal\\r\\n"]);
}

#[test]
fn invalid_later_syntax_and_native_arguments_preflight_before_prefix_effects() {
    for suffix in [
        "cp source target | cat",
        "cp source",
        "not_registered input",
        "cp source target || echo nope",
    ] {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("source"), b"payload").unwrap();
        fs::write(
            root.path().join("commands.txt"),
            format!("touch prefix\n{suffix}\n"),
        )
        .unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_rusty_make"))
            .current_dir(root.path())
            .args(["--commands", "commands.txt"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(!root.path().join("prefix").exists());
        assert!(!root.path().join("target").exists());
    }
}

#[test]
fn native_pipeline_requires_no_command_processes_on_path() {
    let root = tempfile::tempdir().unwrap();
    let empty = tempfile::tempdir().unwrap();
    fs::write(root.path().join("source"), b"native only").unwrap();
    fs::write(root.path().join("commands.txt"), "mkdir -p 'build tree/leaf'\r\ncp sou\\\r\nrce 'build tree/leaf/result' > /dev/null 2> /dev/null\r\ntouch 'build tree/leaf/stamp'\r\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rusty_make"))
        .env("PATH", empty.path())
        .current_dir(root.path())
        .args(["--commands", "commands.txt"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read(root.path().join("build tree/leaf/result")).unwrap(),
        b"native only"
    );
    assert!(root.path().join("build tree/leaf/stamp").is_file());
    assert!(!root.path().join(".rusty_make").exists());
}

#[test]
fn duplicate_registration_does_not_partially_add_aliases() {
    let mut registry = Registry::default();
    assert!(registry.register_external(&["new_alias", "touch"]).is_err());
    assert!(parse("new_alias value\n", &registry).is_err());
    let jobs = parse("touch ordinary\n", &registry).unwrap();
    assert!(
        jobs[0]
            .data()
            .accesses
            .iter()
            .any(|access| access.path == "ordinary" && access.access.writes())
    );
}
