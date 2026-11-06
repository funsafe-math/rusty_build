use std::{fs, process::Command};

fn run(root: &std::path::Path, text: &str) -> std::process::Output {
    fs::write(root.join("commands.txt"), text).unwrap();
    Command::new(env!("CARGO_BIN_EXE_rusty_make"))
        .current_dir(root)
        .args(["--commands", "commands.txt"])
        .output()
        .unwrap()
}

#[test]
fn native_redirections_create_files_and_truncate_before_operation() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("source"), b"original").unwrap();
    fs::write(root.path().join("log"), b"old log").unwrap();
    let output = run(root.path(), "cp source copied > log 2> errors\n");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(root.path().join("copied")).unwrap(), b"original");
    assert_eq!(fs::read(root.path().join("log")).unwrap(), b"");
    assert_eq!(fs::read(root.path().join("errors")).unwrap(), b"");
    let output = run(root.path(), "cp source emptied > source\n");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(root.path().join("emptied")).unwrap(), b"");
}

#[test]
fn redirect_opening_order_and_failure_mask_are_preserved() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("source"), b"payload").unwrap();
    let output = run(
        root.path(),
        "cp source copied > first 2> missing/errors || true\n",
    );
    assert!(output.status.success());
    assert_eq!(fs::read(root.path().join("first")).unwrap(), b"");
    assert!(!root.path().join("copied").exists());
    let output = run(
        root.path(),
        "cp source copied 2> missing/errors > second || :\n",
    );
    assert!(output.status.success());
    assert!(!root.path().join("second").exists());
    assert!(!root.path().join("copied").exists());
}

#[test]
fn explicit_noop_fallback_masks_runtime_failure_not_parse_errors() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("source"), b"continued").unwrap();
    let output = run(
        root.path(),
        "cp missing absent 2>/dev/null || cd .\ncp source final\n",
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!root.path().join("absent").exists());
    assert_eq!(fs::read(root.path().join("final")).unwrap(), b"continued");
    let output = run(root.path(), "touch prefix\ncp || true\n");
    assert!(!output.status.success());
    assert!(!root.path().join("prefix").exists());
}

#[cfg(unix)]
#[test]
fn sed_stdout_and_in_place_forms_have_real_outputs() {
    if !Command::new("sed")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("input file"), b"foo foo\n").unwrap();
    let output = run(
        root.path(),
        "sed 's/foo/bar/g' 'input file' > 'output file'\nsed -i 's/bar/baz/g' 'output file'\n",
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read(root.path().join("input file")).unwrap(),
        b"foo foo\n"
    );
    assert_eq!(
        fs::read(root.path().join("output file")).unwrap(),
        b"baz baz\n"
    );
}

#[test]
fn unproduced_inputs_are_checked_before_any_native_operation() {
    let root = tempfile::tempdir().unwrap();
    let output = run(root.path(), "touch prefix\ncp missing final\n");
    assert!(!output.status.success());
    assert!(!root.path().join("prefix").exists());
    assert!(!root.path().join("final").exists());
}

#[test]
fn sed_same_input_stdout_is_rejected_before_truncation_or_prefix_effects() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("input"), b"original\n").unwrap();
    for form in [
        "sed 's/original/replaced/' input > input",
        "sed -i 's/original/replaced/' input > input",
    ] {
        let output = run(root.path(), &format!("touch prefix\n{form}\n"));
        assert!(!output.status.success());
        assert!(!root.path().join("prefix").exists());
        assert_eq!(fs::read(root.path().join("input")).unwrap(), b"original\n");
    }
}
