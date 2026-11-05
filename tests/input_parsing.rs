use rusty_make::{
    commands::{compiler::Compiler, cp::Cp},
    input::{SourceCommand, read_log},
    registry::{Registry, binary_pattern, command_pattern},
};

fn source(text: &str) -> SourceCommand {
    SourceCommand {
        line: 17,
        text: text.into(),
        target: Some("original-target".into()),
    }
}

#[test]
fn crlf_continuations_match_lf_without_changing_physical_locations_or_literals() {
    let lf = "Makefile:2: target 'result.o' does not exist\ncc -c \\\n 'C:\\source file.c' \\\n -DVALUE='literal\\r\\n' -o result.o\nprintf '%s\\n' done\n";
    let crlf = lf.replace('\n', "\r\n");
    let expected = read_log(lf).unwrap();
    let actual = read_log(&crlf).unwrap();
    assert_eq!(actual.commands().len(), 2);
    for (actual, expected) in actual.commands().iter().zip(expected.commands()) {
        assert_eq!(actual.text, expected.text);
        assert_eq!(actual.line, expected.line);
        assert_eq!(actual.target, expected.target);
        assert_eq!(
            actual.shell().unwrap().program,
            expected.shell().unwrap().program
        );
        assert_eq!(actual.shell().unwrap().args, expected.shell().unwrap().args);
    }
    assert_eq!(actual.commands()[0].line, 2);
    assert_eq!(actual.commands()[1].line, 5);
    assert_eq!(actual.commands()[0].target.as_deref(), Some("result.o"));
    assert_eq!(
        actual.commands()[0].shell().unwrap().args,
        [
            "-c",
            "C:\\source file.c",
            "-DVALUE=literal\\r\\n",
            "-o",
            "result.o"
        ]
    );
    let graph =
        rusty_make::plan_log(actual, std::path::Path::new("."), &Registry::default()).unwrap();
    assert!(graph.jobs[0].typed::<Compiler>().is_some());
    assert_eq!(graph.jobs[0].source.line, 2);
}

#[test]
fn double_quoted_crlf_continuation_is_active_but_literal_escapes_are_unchanged() {
    let log = read_log("tool \"ab\\\r\ncd\" 'literal\\r\\n'\r\ntool next\r\n").unwrap();
    assert_eq!(log.commands()[0].text, "tool \"ab\\\ncd\" 'literal\\r\\n'");
    assert_eq!(
        log.commands()[0].shell().unwrap().args,
        ["abcd", "literal\\r\\n"]
    );
    assert_eq!(log.commands()[1].line, 3);
}

#[test]
fn unquoted_continuations_join_words_without_inventing_argument_boundaries() {
    for (spelling, expected) in [
        ("to\\\nol se\\\ned", vec!["seed"]),
        ("tool 'se'\\\n'ed'", vec!["seed"]),
        ("tool se\\\n\\\ned", vec!["seed"]),
        ("tool se\\\n ed", vec!["se", "ed"]),
    ] {
        for newline in ["\n", "\r\n"] {
            let text = format!("{}\n", spelling).replace('\n', newline);
            let log = read_log(&text).unwrap();
            let shell = log.commands()[0].shell().unwrap();
            assert_eq!(shell.program, "tool");
            assert_eq!(shell.args, expected);
        }
    }
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("seed"), b"joined operand").unwrap();
    let graph = rusty_make::parse_plan(
        "cp se\\\r\ned result\r\n",
        root.path(),
        &Registry::default(),
    )
    .unwrap();
    rusty_make::scheduler::run(&graph, &Default::default()).unwrap();
    assert_eq!(
        std::fs::read(root.path().join("result")).unwrap(),
        b"joined operand"
    );
}

#[test]
fn edited_source_is_classified_afresh_instead_of_reusing_parsed_argv() {
    let log = read_log("cp original destination\n").unwrap();
    let mut edited = log.commands()[0].source().clone();
    edited.text = "cp replacement destination".into();
    let record = Registry::default().classify(edited).unwrap();
    assert!(record.typed::<Cp>().is_some());
    assert_eq!(record.shell.unwrap().args, ["replacement", "destination"]);
    assert_eq!(
        log.commands()[0].shell().unwrap().args,
        ["original", "destination"]
    );
}

#[test]
fn opaque_records_never_execute_substitutions_or_join_incomplete_records() {
    let directory = tempfile::tempdir().unwrap();
    let sentinel = directory.path().join("must-not-exist");
    let record = format!(
        "echo $(touch {})\n",
        shell_words::quote(sentinel.to_str().unwrap())
    );
    let text = record.repeat(256);
    let log = read_log(&text).unwrap();
    assert_eq!(log.commands().len(), 256);
    assert!(log.commands().iter().all(|record| record.shell().is_none()));
    assert_eq!(log.commands()[255].line, 256);
    assert!(!sentinel.exists());

    for malformed in [
        "echo $HOME\nthen\necho $HOME\n",
        "echo $HOME\necho ;;\necho $HOME\n",
        "echo $HOME\ncat <<EOF\nEOF\n",
        "if true; then\necho yes\nfi\n",
        "echo $HOME\n) (\necho $HOME\n",
        "echo $HOME\necho 'unterminated\n'\n",
    ] {
        assert!(
            read_log(malformed).is_err(),
            "accepted malformed records: {malformed:?}"
        );
    }
    assert!(!sentinel.exists());
}

#[test]
fn opaque_log_planning_does_not_need_bash_on_path() {
    let directory = tempfile::tempdir().unwrap();
    let commands = directory.path().join("commands.txt");
    std::fs::write(
        &commands,
        "echo $HOME\nfalse || echo yes\nfor x in a b; do echo \"$x\"; done\n",
    )
    .unwrap();
    // Change PATH only in the CLI child, never the test runner's global
    // environment. No executable (including Bash) exists in this directory.
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_rusty_make"))
        .current_dir(directory.path())
        .env("PATH", directory.path())
        .args(["--dry-run", "--no-optimize", "--commands"])
        .arg(&commands)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("Parsed 3 commands"));
}

#[test]
fn permissive_grammar_shapes_do_not_hide_malformed_bash() {
    for text in [
        "then",
        "done",
        "else",
        "coproc",
        "{ }",
        "if true; then fi",
        "if true; then echo yes; else fi",
        "for x in a; do done",
        "while true; do done",
        "tool (echo invalid)",
        "cat <<EOF\nbody",
        "cat <<EOF\nbody\nEOFextra\n",
        "cat <<EOF\nbody\n EOF\n",
        "cat <<-EOF\nbody\n EOF\n",
    ] {
        assert!(
            rusty_make::shell::parse(text).is_err(),
            "accepted malformed Bash: {text:?}"
        );
    }
    for text in [
        "if true; then echo yes; else echo no; fi",
        "for x in a; do echo \"$x\"; done",
        "cat <<EOF\nbody\nEOF\n",
        "cat <<'EOF'\nbody\nEOF\n",
        "cat <<-EOF\nbody\n\tEOF\n",
    ] {
        assert!(
            rusty_make::shell::parse(text).unwrap().is_none(),
            "{text:?}"
        );
    }
}

#[test]
fn later_malformed_record_rejects_the_whole_plan_before_prefix_effects() {
    let directory = tempfile::tempdir().unwrap();
    let text = "mkdir prefix-effect\necho $HOME\nthen\n";
    assert!(rusty_make::parse_plan(text, directory.path(), &Registry::default()).is_err());
    assert!(!directory.path().join("prefix-effect").exists());
}

#[test]
fn executable_suffixes_and_forward_slash_windows_paths_keep_typed_identity() {
    let registry = Registry::default();
    for text in [
        "cp.exe a b",
        "C:/tools/cp.exe a b",
        "'C:/Program Files/tools/cp.exe' a b",
        "\"C:/Program Files/tools/cp.exe\" a b",
    ] {
        let record = registry.classify(source(text)).unwrap();
        assert!(record.typed::<Cp>().is_some(), "not typed: {text}");
        assert_eq!(record.source.text, text);
        assert_eq!(record.source.line, 17);
        assert_eq!(record.source.target.as_deref(), Some("original-target"));
    }
    let record = registry
        .classify(source("C:/tools/gcc.exe -c source.c -o source.o"))
        .unwrap();
    assert!(record.typed::<Compiler>().is_some());
    let matcher = regex::Regex::new(&binary_pattern("objcopy")).unwrap();
    assert!(matcher.is_match("objcopy.exe a b"));
    assert!(!matcher.is_match("objcopy.exe.exe a b"));
    assert!(
        !regex::Regex::new(&command_pattern("cp"))
            .unwrap()
            .is_match("cp.exe.exe a b")
    );
}

#[test]
fn backslash_executable_paths_follow_host_and_bash_quoting() {
    let registry = Registry::default();
    for text in [
        "'C:\\tools\\cp.exe' a b",
        "\"C:\\tools\\cp.exe\" a b",
        "C:\\\\tools\\\\cp.exe a b",
    ] {
        let record = registry.classify(source(text)).unwrap();
        assert_eq!(record.typed::<Cp>().is_some(), cfg!(windows), "{text}");
        assert_eq!(record.shell.unwrap().program, "C:\\tools\\cp.exe");
    }
    let record = registry.classify(source("C:\\tools\\cp.exe a b")).unwrap();
    assert!(record.typed::<Cp>().is_none());
    assert_eq!(record.shell.unwrap().program, "C:toolscp.exe");
}

#[test]
fn failed_pattern_replacement_preserves_existing_rules_and_overlaps_stay_fatal() {
    let mut registry = Registry::default();
    assert!(registry.replace_patterns::<Cp>(&["^custom", "["]).is_err());
    assert!(
        registry
            .classify(source("cp a b"))
            .unwrap()
            .typed::<Cp>()
            .is_some()
    );
    registry
        .register_external("overlap", &["^cp", "cp"])
        .unwrap();
    let error = registry.classify(source("cp a b")).unwrap_err().to_string();
    assert!(error.contains("3 matching patterns"), "{error}");
    assert!(error.contains("overlap[0]: ^cp"), "{error}");
    assert!(error.contains("overlap[1]: cp"), "{error}");
}

#[test]
fn sole_cd_dot_fallback_exposes_static_left_command_without_rewriting_source() {
    for text in [
        "coan source --replace file.c || cd .",
        "coan source --replace file.c > /dev/null || cd .",
        "coan source --replace file.c 2> /dev/null || cd .",
        "coan source --replace file.c > /dev/null 2> /dev/null || cd .",
        "coan source --replace file.c 2>/dev/null >/dev/null || cd .",
    ] {
        let log = read_log(text).unwrap();
        let record = &log.commands()[0];
        assert_eq!(record.text, text);
        let command = record.shell().unwrap();
        assert_eq!(command.program, "coan");
        assert_eq!(command.args, ["source", "--replace", "file.c"]);
        assert!(command.noop_fallback);
        assert!(command.stdin.is_none());
        for path in [&command.stdout, &command.stderr].into_iter().flatten() {
            assert_eq!(path, std::path::Path::new("/dev/null"));
        }
    }
    let command = rusty_make::shell::parse("coan source file.c > /dev/null 2> /dev/null")
        .unwrap()
        .unwrap();
    assert!(!command.noop_fallback);
    assert_eq!(
        command.stdout.as_deref(),
        Some(std::path::Path::new("/dev/null"))
    );
    assert_eq!(
        command.stderr.as_deref(),
        Some(std::path::Path::new("/dev/null"))
    );
}

#[test]
fn cd_dot_metadata_never_strips_quoted_text_or_general_control_flow() {
    let command = rusty_make::shell::parse("tool '|| cd .'").unwrap().unwrap();
    assert_eq!(command.args, ["|| cd ."]);
    assert!(!command.noop_fallback);
    for text in [
        "tool || cd ..",
        "tool || cd ./",
        "tool || cd .;",
        "tool || 'cd' '.'",
        "(tool) || cd .",
        "tool | cat || cd .",
        "VALUE=x tool || cd .",
        "tool && cd .",
        "tool || command cd .",
        "tool || cd . && next",
        "tool || cd . > log",
        "tool > out 2> err || cd .",
        "tool >/dev/null >/dev/null || cd .",
        "tool >/dev/null 2>&1 || cd .",
    ] {
        assert!(
            rusty_make::shell::parse(text).unwrap().is_none(),
            "must retain opaque shell semantics: {text}"
        );
    }
}

#[test]
fn noop_fallback_metadata_defaults_false_for_existing_serialized_commands() {
    let command: rusty_make::shell::ShellCommand = serde_json::from_str(
        r#"{"program":"tool","args":[],"stdin":null,"stdout":null,"stderr":null,"append_stdout":false}"#,
    ).unwrap();
    assert!(!command.noop_fallback);
    let command = rusty_make::shell::parse("tool || cd .").unwrap().unwrap();
    let encoded = serde_json::to_string(&command).unwrap();
    let restored: rusty_make::shell::ShellCommand = serde_json::from_str(&encoded).unwrap();
    assert!(restored.noop_fallback);
}
