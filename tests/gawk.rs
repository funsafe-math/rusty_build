use rusty_make::{
    commands::gawk::Gawk, graph::create_graph, job::FileAccess, parse, registry::Registry,
    scheduler::execute,
};
use std::{any::Any, fs, process::Command};

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).into()).collect()
}
fn job(words: &[&str]) -> Gawk {
    Gawk::parse(&argv(words), &Default::default()).unwrap()
}
fn available() -> bool {
    let found = Command::new("gawk").arg("--version").output().is_ok();
    if !found {
        eprintln!("skipping unavailable gawk");
    }
    found
}

#[test]
fn distinguishes_inline_programs_script_files_data_and_assignments() {
    let args = argv(&[
        "gawk",
        "-F:",
        "--field-separator=[\\:]",
        "-v",
        "first=one",
        "--assign=second=two",
        "-f",
        "script file.awk",
        "--file=other.awk",
        "-e",
        "BEGIN { print \"literal\\bytes\" }",
        "--source=END { print \"done\" }",
        "data file",
        "count=2",
        "-",
        "invalid-name=value",
    ]);
    let job = Gawk::parse(&args, &Default::default()).unwrap();
    assert_eq!(job.argv, args);
    assert_eq!(job.field_separators, [":", "[\\:]"]);
    assert_eq!(job.assignments, ["first=one", "second=two"]);
    assert_eq!(job.script_files, ["script file.awk", "other.awk"]);
    assert_eq!(
        job.programs,
        [
            "BEGIN { print \"literal\\bytes\" }",
            "END { print \"done\" }"
        ]
    );
    assert_eq!(
        job.operands,
        ["data file", "count=2", "-", "invalid-name=value"]
    );
    assert_eq!(job.data_files, ["data file", "invalid-name=value"]);
    assert_eq!(
        job.data.accesses,
        [
            FileAccess::read("script file.awk"),
            FileAccess::read("other.awk"),
            FileAccess::read("data file"),
            FileAccess::read("invalid-name=value")
        ]
    );
    assert!(job.metadata_complete);
    assert!(job.data.unknown_effects);
}

#[test]
fn empty_programs_attached_values_and_separators_preserve_operand_meanings() {
    let parsed = job(&[
        "gawk",
        "-F,",
        "-vname=value",
        "-e",
        "",
        "-f-",
        "--",
        "-input",
        "name=value",
        "-",
    ]);
    assert_eq!(parsed.field_separators, [","]);
    assert_eq!(parsed.assignments, ["name=value"]);
    assert_eq!(parsed.programs, [""]);
    assert_eq!(parsed.script_files, ["-"]);
    assert_eq!(parsed.data.accesses, [FileAccess::read("-input")]);
    let parsed = job(&[
        "gawk",
        "--source=",
        "--field-separator=",
        "--",
        "-input",
        "-v",
    ]);
    assert_eq!(parsed.operands, ["-input", "-v"]);
    assert_eq!(
        parsed.data.accesses,
        [FileAccess::read("-input"), FileAccess::read("-v")]
    );
}

#[test]
fn unknown_option_arity_does_not_invent_inputs_or_outputs() {
    let args = argv(&[
        "gawk",
        "-f",
        "known.awk",
        "--future-option",
        "possible value",
        "possible program",
        "-vname=literal",
        "data",
    ]);
    let parsed = Gawk::parse(&args, &Default::default()).unwrap();
    assert!(!parsed.metadata_complete);
    assert_eq!(parsed.data.accesses, [FileAccess::read("known.awk")]);
    assert!(parsed.programs.is_empty());
    assert!(parsed.operands.is_empty());
    assert_eq!(parsed.argv, args);
    let parsed = job(&["gawk", "BEGIN { print \"hidden\" > \"side-effect\" }"]);
    assert!(parsed.data.accesses.is_empty());
    assert!(parsed.data.unknown_effects);
}

#[test]
fn malformed_known_options_fail_during_parsing_without_execution() {
    for args in [
        vec!["gawk"],
        vec!["gawk", "-f"],
        vec!["gawk", "--file="],
        vec!["gawk", "-e"],
        vec!["gawk", "-F"],
        vec!["gawk", "--assign"],
    ] {
        assert!(
            Gawk::parse(&argv(&args), &Default::default()).is_err(),
            "{args:?}"
        );
    }
    for command in ["gawk --help", "gawk --version", "gawk --copyright"] {
        assert!(parse(command, &Registry::default()).is_ok());
    }
}

#[test]
fn registered_aliases_are_exact_and_consumer_can_downcast() {
    for executable in ["gawk", "awk", "/usr/bin/gawk", "gawk.exe"] {
        let jobs = parse(
            &format!("{executable} 'BEGIN {{ exit 0 }}'"),
            &Registry::default(),
        )
        .unwrap();
        let parsed = (jobs[0].as_ref() as &dyn Any)
            .downcast_ref::<Gawk>()
            .unwrap();
        assert_eq!(parsed.argv[0], executable);
        assert!(parsed.data.unknown_effects);
    }
    assert!(parse("gawk-wrapper 'BEGIN {}'", &Registry::default()).is_err());
}

#[test]
fn installed_gawk_executes_inline_code_and_redirections() {
    if !available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let p = root.path().display();
    fs::write(format!("{p}/data file"), "a:one\nb:two\n").unwrap();
    fs::write(format!("{p}/output"), "stale content").unwrap();
    let text = format!(
        "gawk -F: -v prefix=tag '{{ print prefix \":\" $2 }}' '{p}/data file' > '{p}/output'\ngawk '{{ print toupper($0) }}' '{p}/output' > '{p}/copied' 2> '{p}/errors'"
    );
    let dag = create_graph(parse(&text, &Registry::default()).unwrap()).unwrap();
    assert_eq!(dag.waves, [vec![0], vec![1]]);
    dag.check_inputs().unwrap();
    execute(&dag, false).unwrap();
    assert_eq!(
        fs::read(format!("{p}/output")).unwrap(),
        b"tag:one\ntag:two\n"
    );
    assert_eq!(
        fs::read(format!("{p}/copied")).unwrap(),
        b"TAG:ONE\nTAG:TWO\n"
    );
    assert_eq!(fs::read(format!("{p}/errors")).unwrap(), b"");
}

#[test]
fn installed_gawk_executes_script_bytes_unchanged() {
    if !available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let p = root.path().display();
    let script = b"BEGIN { printf \"backslash=\\\\ quote=\\\"\\n\" }\n{ print $2 }\n";
    fs::write(format!("{p}/script file.awk"), script).unwrap();
    fs::write(format!("{p}/data"), "left,right\n").unwrap();
    let text =
        format!("gawk --file='{p}/script file.awk' --field-separator=, '{p}/data' > '{p}/output'");
    let dag = create_graph(parse(&text, &Registry::default()).unwrap()).unwrap();
    dag.check_inputs().unwrap();
    execute(&dag, false).unwrap();
    assert_eq!(
        fs::read(format!("{p}/output")).unwrap(),
        b"backslash=\\ quote=\"\nright\n"
    );
    assert_eq!(fs::read(format!("{p}/script file.awk")).unwrap(), script);
}

#[test]
fn installed_gawk_hidden_writes_are_ordered_without_invented_outputs() {
    if !available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let p = root.path().display();
    let text = format!(
        "gawk 'BEGIN {{ print \"hidden\" > \"{p}/side-effect\"; close(\"{p}/side-effect\") }}'\ngawk 'BEGIN {{ while ((getline line < \"{p}/side-effect\") > 0) print line }}' > '{p}/side-copy'"
    );
    let dag = create_graph(parse(&text, &Registry::default()).unwrap()).unwrap();
    assert!(dag.jobs[0].data().accesses.is_empty());
    assert!(dag.jobs[0].data().unknown_effects);
    assert_eq!(dag.waves, [vec![0], vec![1]]);
    execute(&dag, false).unwrap();
    assert_eq!(fs::read(format!("{p}/side-copy")).unwrap(), b"hidden\n");
}
