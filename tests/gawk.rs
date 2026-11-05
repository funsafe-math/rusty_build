use rusty_make::{
    commands::gawk::Gawk,
    graph::BuildGraph,
    input::SourceCommand,
    job::{ExecutionStrategy, Job, JobRecord},
    parse_plan,
    registry::Registry,
    scheduler::{RunOptions, run},
};
use std::{fs, path::PathBuf};

fn classify(text: &str) -> JobRecord {
    Registry::default()
        .classify(SourceCommand {
            line: 7,
            text: text.into(),
            target: Some("original-target".into()),
        })
        .unwrap()
}

#[test]
fn matches_only_gawk_program_and_path_spellings() {
    for program in [
        "gawk",
        "/usr/bin/gawk",
        "gawk.exe",
        "C:/tools/gawk.exe",
        "'tools with spaces/gawk'",
    ] {
        let text = format!("{program} 'BEGIN {{ exit 0 }}'");
        let record = classify(&text);
        assert_eq!(record.spec.kind, "gawk", "{text}");
        assert_eq!(record.spec.type_tag, "gawk");
        assert_eq!(record.source.text, text);
        assert_eq!(record.source.line, 7);
        assert_eq!(record.source.target.as_deref(), Some("original-target"));
        assert_eq!(record.strategy, ExecutionStrategy::External);
        assert!(record.spec.unknown_effects && !record.spec.cacheable);
    }
    if cfg!(windows) {
        assert_eq!(
            classify(r#"'C:\Program Files\gawk.exe' 'BEGIN {}'"#)
                .spec
                .kind,
            "gawk"
        );
    }
    for program in [
        "awk",
        "gawk-wrapper",
        "notgawk",
        "gawk.exe.extra",
        "gawk.exe.exe",
    ] {
        assert_eq!(
            classify(&format!("{program} 'BEGIN {{}}'")).spec.kind,
            "unknown"
        );
    }
}

#[test]
fn distinguishes_programs_script_files_data_and_assignments() {
    let record = classify(
        r#"gawk -F: --field-separator='[\\:]' -v first=one --assign=second=two -f 'script file.awk' --file=other.awk -e 'BEGIN { print "literal\\bytes" }' --source='END { print "done" }' 'data file' count=2 - invalid-name=value"#,
    );
    let job = record.typed::<Gawk>().unwrap();
    assert_eq!(job.field_separators, [":", r"[\\:]"]);
    assert_eq!(job.assignments, ["first=one", "second=two"]);
    assert_eq!(
        job.programs,
        [
            r#"BEGIN { print "literal\\bytes" }"#,
            r#"END { print "done" }"#
        ]
    );
    assert_eq!(
        job.script_files,
        [PathBuf::from("script file.awk"), PathBuf::from("other.awk")]
    );
    assert_eq!(
        job.operands,
        ["data file", "count=2", "-", "invalid-name=value"]
    );
    assert_eq!(
        job.data_files,
        [
            PathBuf::from("data file"),
            PathBuf::from("invalid-name=value")
        ]
    );
    assert_eq!(
        record.spec.inputs,
        [
            PathBuf::from("script file.awk"),
            PathBuf::from("other.awk"),
            PathBuf::from("data file"),
            PathBuf::from("invalid-name=value"),
        ]
    );
    assert!(record.spec.outputs.is_empty());
    assert!(job.metadata_complete);
    assert_eq!(
        job.argv,
        std::iter::once("gawk".to_owned())
            .chain(record.shell.as_ref().unwrap().args.iter().cloned())
            .collect::<Vec<_>>()
    );
}

#[test]
fn attached_values_empty_programs_and_operand_separator_are_preserved() {
    let record = classify("gawk -F, -vname=value -e '' -f- -- -input name=value -");
    let job = record.typed::<Gawk>().unwrap();
    assert_eq!(job.field_separators, [","]);
    assert_eq!(job.assignments, ["name=value"]);
    assert_eq!(job.programs, [""]);
    assert_eq!(job.script_files, [PathBuf::from("-")]);
    assert_eq!(job.data_files, [PathBuf::from("-input")]);

    let record = classify("gawk --source='' --field-separator='' -- -input -v");
    let job = record.typed::<Gawk>().unwrap();
    assert_eq!(job.programs, [""]);
    assert_eq!(job.field_separators, [""]);
    assert_eq!(job.operands, ["-input", "-v"]);
    assert_eq!(
        job.data_files,
        [PathBuf::from("-input"), PathBuf::from("-v")]
    );

    let record = classify("gawk -f- data");
    let job = record.typed::<Gawk>().unwrap();
    assert_eq!(job.script_files, [PathBuf::from("-")]);
    assert_eq!(
        job.inputs().collect::<Vec<_>>(),
        [std::path::Path::new("data")]
    );

    let record = classify("gawk -- '' data -v");
    let job = record.typed::<Gawk>().unwrap();
    assert_eq!(job.programs, [""]);
    assert_eq!(job.operands, ["data", "-v"]);
}

#[test]
fn unknown_option_arity_is_not_invented_as_precise_metadata() {
    let record = classify(
        r#"gawk -f known.awk --future-option 'possibly an option value' 'possibly a program' -vname=literal data"#,
    );
    let job = record.typed::<Gawk>().unwrap();
    assert!(!job.metadata_complete);
    assert_eq!(job.script_files, [PathBuf::from("known.awk")]);
    assert!(job.programs.is_empty());
    assert!(job.assignments.is_empty());
    assert!(job.operands.is_empty() && job.data_files.is_empty());
    assert_eq!(job.argv[3], "--future-option");
    assert_eq!(job.argv[4], "possibly an option value");
    assert_eq!(job.argv[6], "-vname=literal");
    assert!(job.unknown_effects() && !job.cacheable());
}

#[test]
fn malformed_known_forms_abort_planning_before_any_prefix_executes() {
    let root = tempfile::tempdir().unwrap();
    for malformed in [
        "gawk",
        "gawk -f",
        "gawk --file",
        "gawk --file=",
        "gawk -e",
        "gawk --source",
        "gawk -F",
        "gawk --field-separator",
        "gawk -v",
        "gawk --assign",
        "gawk -f script.awk --assign",
    ] {
        let result = parse_plan(
            &format!("touch prefix-executed\n{malformed}\n"),
            root.path(),
            &Registry::default(),
        );
        assert!(result.is_err(), "{malformed}");
        assert!(!root.path().join("prefix-executed").exists());
    }
    for informational in ["gawk --help", "gawk --version", "gawk --copyright"] {
        assert_eq!(classify(informational).spec.kind, "gawk");
    }
}

#[test]
fn real_gawk_executes_inline_code_and_all_simple_redirections() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("data file"), "a:one\nb:two\n").unwrap();
    fs::write(p.join("output"), "must be overwritten\n").unwrap();
    let registry = Registry::default();
    let graph = parse_plan(
        "gawk -F: -v prefix=tag '{ print prefix \":\" $2 }' < 'data file' > output\ngawk --source='BEGIN { print \"tail\" }' >> output\ngawk '{ print toupper($0) }' output > copied 2> errors\n",
        p,
        &registry,
    )
    .unwrap();
    assert!(
        graph
            .jobs
            .iter()
            .all(|record| record.spec.unknown_effects && !record.spec.cacheable)
    );
    let bytes = serde_json::to_vec(&graph).unwrap();
    let restored = BuildGraph::from_json(&bytes, &registry).unwrap();
    run(&restored, &RunOptions::default()).unwrap();
    assert_eq!(
        fs::read(p.join("output")).unwrap(),
        b"tag:one\ntag:two\ntail\n"
    );
    assert_eq!(
        fs::read(p.join("copied")).unwrap(),
        b"TAG:ONE\nTAG:TWO\nTAIL\n"
    );
    assert_eq!(fs::read(p.join("errors")).unwrap(), b"");
}

#[test]
fn real_gawk_executes_script_files_without_reinterpreting_their_bytes() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    let script = br#"BEGIN { printf "backslash=\\ quote=\"\n" }
{ print $2 }
"#;
    fs::write(p.join("script file.awk"), script).unwrap();
    fs::write(p.join("data"), "left,right\n").unwrap();
    let graph = parse_plan(
        "gawk --file='script file.awk' --field-separator=, data > output\n",
        p,
        &Registry::default(),
    )
    .unwrap();
    let job = graph.jobs[0].typed::<Gawk>().unwrap();
    assert_eq!(job.script_files, [PathBuf::from("script file.awk")]);
    assert_eq!(job.data_files, [PathBuf::from("data")]);
    run(&graph, &RunOptions::default()).unwrap();
    assert_eq!(
        fs::read(p.join("output")).unwrap(),
        b"backslash=\\ quote=\"\nright\n"
    );
    assert_eq!(fs::read(p.join("script file.awk")).unwrap(), script);
}

#[test]
fn real_gawk_in_place_and_hidden_script_writes_remain_ordered_barriers() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("seed"), "old value\n").unwrap();
    let graph = parse_plan(
        "cp seed mutable\ngawk -i inplace '{ gsub(/old/, \"new\"); print }' mutable\ncp mutable copied\ngawk 'BEGIN { print \"hidden\" > \"side-effect\"; close(\"side-effect\") }'\ncp side-effect side-copy\n",
        p,
        &Registry::default(),
    )
    .unwrap();
    assert_eq!(
        graph.waves,
        vec![vec![0], vec![1], vec![2], vec![3], vec![4]]
    );
    let inplace = graph.jobs[1].typed::<Gawk>().unwrap();
    assert!(!inplace.metadata_complete);
    assert_eq!(inplace.argv[1..3], ["-i", "inplace"]);
    assert!(graph.jobs[3].spec.outputs.is_empty());
    assert!(graph.jobs[3].spec.unknown_effects);
    run(
        &graph,
        &RunOptions {
            jobs: 4,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(fs::read(p.join("mutable")).unwrap(), b"new value\n");
    assert_eq!(fs::read(p.join("copied")).unwrap(), b"new value\n");
    assert_eq!(fs::read(p.join("side-copy")).unwrap(), b"hidden\n");
}

#[test]
fn real_gawk_exit_status_and_script_errors_are_not_inferred_from_output() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    let graph = parse_plan(
        "gawk 'BEGIN { print \"before failure\"; exit 23 }' > output 2> errors\n",
        p,
        &Registry::default(),
    )
    .unwrap();
    assert!(run(&graph, &RunOptions::default()).is_err());
    assert_eq!(fs::read(p.join("output")).unwrap(), b"before failure\n");
    let graph = parse_plan(
        "gawk 'BEGIN {' > invalid-output 2> syntax-errors\n",
        p,
        &Registry::default(),
    )
    .unwrap();
    assert!(run(&graph, &RunOptions::default()).is_err());
    assert!(!fs::read(p.join("syntax-errors")).unwrap().is_empty());
}
