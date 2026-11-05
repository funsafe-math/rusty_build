use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use rusty_make::commands::coan::Coan;
use rusty_make::input::SourceCommand;
use rusty_make::job::{
    BatchContext, BatchItem, ExecutionContext, ExecutionStrategy, Job, JobRecord,
};
use rusty_make::registry::Registry;
use rusty_make::shell;

const CONDITIONAL: &str = "#ifdef FEATURE\nkept\n#else\ndropped\n#endif\n";

// Optional real-tool tests use coan from PATH, or the executable explicitly
// selected by RUSTY_MAKE_COAN. An invalid explicit selection is a test failure.
fn reference() -> Option<String> {
    let explicit = std::env::var("RUSTY_MAKE_COAN").ok();
    let program = explicit.clone().unwrap_or_else(|| "coan".into());
    match Command::new(&program).arg("--version").output() {
        Ok(output) if output.status.success() => Some(program),
        result if explicit.is_some() => {
            panic!("RUSTY_MAKE_COAN must select a real coan executable: {result:?}")
        }
        _ => {
            eprintln!("skipping real coan test: install coan on PATH or set RUSTY_MAKE_COAN");
            None
        }
    }
}

fn source(text: &str) -> SourceCommand {
    SourceCommand {
        line: 1,
        text: text.into(),
        target: None,
    }
}

fn record(root: &Path, program: &str, args: &[&str], redirect: &str) -> JobRecord {
    let text = std::iter::once(program)
        .chain(args.iter().copied())
        .map(|arg| shell_words::quote(arg).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
        + redirect;
    let shell = shell::parse(&text).unwrap().unwrap();
    let job = Coan::parse(&shell).unwrap();
    let mut record = JobRecord::new(source(&text), Some(shell), job).unwrap();
    record.refine_effects(root).unwrap();
    record
}

fn execute(record: &JobRecord, root: &Path) -> anyhow::Result<()> {
    record
        .typed::<Coan>()
        .unwrap()
        .execute(&ExecutionContext { cwd: root, record })
}

fn batch(
    records: &[JobRecord],
    root: &Path,
    cancelled: &AtomicBool,
) -> rusty_make::job::BatchReport {
    let items: Vec<_> = records
        .iter()
        .enumerate()
        .map(|(id, record)| BatchItem {
            id,
            job: record.typed::<Coan>().unwrap(),
            record,
        })
        .collect();
    Coan::execute_batch(
        &items,
        &BatchContext {
            cwd: root,
            cancelled,
        },
    )
}

#[test]
fn anchored_matchers_and_conservative_argv_preserve_original_commands() {
    let registry = Registry::default();
    for program in [
        "coan",
        "coan.exe",
        "/opt/bin/coan",
        "'tools with spaces/coan.exe'",
    ] {
        let text = format!("{program} source --future-option 'x y.c'");
        let record = registry.classify(source(&text)).unwrap();
        assert_eq!(record.spec.kind, "coan", "{text}");
        assert_eq!(record.strategy, ExecutionStrategy::External);
        assert!(record.spec.unknown_effects);
        assert!(!record.spec.cacheable);
        assert_eq!(record.source.text, text);
        assert_eq!(
            record.typed::<Coan>().unwrap().argv[1..],
            ["source", "--future-option", "x y.c"]
        );
    }
    for program in ["coan-wrapper", "notcoan", "coan.exe.extra"] {
        assert_eq!(
            registry
                .classify(source(&format!("{program} source file.c")))
                .unwrap()
                .spec
                .kind,
            "unknown"
        );
    }
    let windows_path = registry
        .classify(source("'C:\\tools\\coan.exe' source file.c"))
        .unwrap();
    assert_eq!(
        windows_path.spec.kind,
        if cfg!(windows) { "coan" } else { "unknown" }
    );
}

#[test]
fn only_explicit_closed_replacement_files_have_precise_effects() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("file.c"), CONDITIONAL).unwrap();
    let precise = record(
        root.path(),
        "coan",
        &[
            "source",
            "-r",
            "--define",
            "FEATURE=1",
            "--discard=drop",
            "file.c",
        ],
        "",
    );
    #[cfg(unix)]
    {
        assert!(!precise.spec.unknown_effects);
        assert_eq!(precise.spec.inputs, [PathBuf::from("file.c")]);
        assert_eq!(precise.spec.outputs, [PathBuf::from("file.c")]);
    }
    assert!(!precise.spec.cacheable);
    for args in [
        vec!["source", "-DFEATURE=1", "file.c"],
        vec!["source", "-r", "-DFEATURE=1"],
        vec!["source", "-r", "--backup=.bak", "file.c"],
        vec!["source", "-r", "--recurse", "file.c"],
        vec!["source", "-r", "--file", "settings", "file.c"],
        vec!["source", "-r", "--future-option", "file.c"],
        vec!["symbols", "-DFEATURE=1", "file.c"],
    ] {
        let job = record(root.path(), "coan", &args, "");
        assert!(job.spec.unknown_effects, "{args:?}");
        assert!(!job.spec.cacheable);
    }
    let redirected = record(
        root.path(),
        "coan",
        &["source", "-r", "-DFEATURE=1", "file.c"],
        " > report",
    );
    assert!(redirected.spec.unknown_effects);
    assert_eq!(redirected.spec.outputs, [PathBuf::from("report")]);
}

#[test]
fn real_changed_exit_17_is_success_and_changes_the_requested_file() {
    let Some(coan) = reference() else { return };
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("reference.c"), CONDITIONAL).unwrap();
    fs::write(root.path().join("typed.c"), CONDITIONAL).unwrap();
    let status = Command::new(&coan)
        .current_dir(root.path())
        .args(["source", "-r", "-DFEATURE=1", "reference.c"])
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(17), "real coan changed-source status");
    let job = record(
        root.path(),
        &coan,
        &["source", "-r", "-DFEATURE=1", "typed.c"],
        "",
    );
    execute(&job, root.path()).unwrap();
    assert_eq!(
        fs::read_to_string(root.path().join("typed.c")).unwrap(),
        "kept\n"
    );
}

#[test]
fn real_plain_source_reporting_and_simple_redirects_execute_directly() {
    let Some(coan) = reference() else { return };
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("input.c"), CONDITIONAL).unwrap();
    fs::write(
        root.path().join("output.txt"),
        "old output must be truncated\n",
    )
    .unwrap();
    for redirect in [" > output.txt", " >> output.txt"] {
        execute(
            &record(
                root.path(),
                &coan,
                &["source", "-DFEATURE=1", "input.c"],
                redirect,
            ),
            root.path(),
        )
        .unwrap();
    }
    assert_eq!(
        fs::read_to_string(root.path().join("output.txt")).unwrap(),
        "kept\nkept\n"
    );
    execute(
        &record(
            root.path(),
            &coan,
            &["symbols", "input.c"],
            " > symbols.txt",
        ),
        root.path(),
    )
    .unwrap();
    assert!(
        fs::read_to_string(root.path().join("symbols.txt"))
            .unwrap()
            .contains("FEATURE")
    );
    let bad = record(
        root.path(),
        &coan,
        &["source", "--not-a-real-coan-option"],
        " 2> errors.txt",
    );
    assert!(execute(&bad, root.path()).is_err());
    assert!(!fs::read(root.path().join("errors.txt")).unwrap().is_empty());
}

#[test]
fn real_stdin_filename_list_preserves_spaces_and_remains_conservative() {
    let Some(coan) = reference() else { return };
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("one spaced.c"), CONDITIONAL).unwrap();
    fs::write(root.path().join("two spaced.c"), CONDITIONAL).unwrap();
    fs::write(
        root.path().join("list.txt"),
        "\"one spaced.c\"\n\"two spaced.c\"\n",
    )
    .unwrap();
    let job = record(
        root.path(),
        &coan,
        &["source", "-r", "-DFEATURE=1"],
        " < list.txt",
    );
    assert!(job.spec.unknown_effects);
    execute(&job, root.path()).unwrap();
    for name in ["one spaced.c", "two spaced.c"] {
        assert_eq!(
            fs::read_to_string(root.path().join(name)).unwrap(),
            "kept\n"
        );
    }
}

#[test]
fn real_batch_quotes_spaced_names_and_never_mixes_different_settings() {
    let Some(coan) = reference() else { return };
    let root = tempfile::tempdir().unwrap();
    for name in ["first spaced.c", "second.c", "third spaced.c"] {
        fs::write(root.path().join(name), CONDITIONAL).unwrap();
    }
    let records = [
        record(
            root.path(),
            &coan,
            &["source", "-r", "-DFEATURE=1", "first spaced.c"],
            "",
        ),
        record(
            root.path(),
            &coan,
            &["source", "-r", "-UFEATURE", "second.c"],
            "",
        ),
        record(
            root.path(),
            &coan,
            &["source", "-r", "-DFEATURE=1", "third spaced.c"],
            "",
        ),
    ];
    let cancelled = AtomicBool::new(false);
    let report = batch(&records, root.path(), &cancelled);
    assert_eq!(report.outcomes.len(), 3);
    assert!(report.outcomes.iter().all(|(_, result)| result.is_ok()));
    assert!(!cancelled.load(Ordering::Acquire));
    assert_eq!(
        fs::read_to_string(root.path().join("first spaced.c")).unwrap(),
        "kept\n"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("second.c")).unwrap(),
        "dropped\n"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("third spaced.c")).unwrap(),
        "kept\n"
    );
}

#[cfg(unix)]
#[test]
fn real_aggregate_batch_error_certifies_no_per_file_successes() {
    let Some(coan) = reference() else { return };
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("good.c"), CONDITIONAL).unwrap();
    fs::write(root.path().join("bad.c"), "#ifdef FEATURE\nunterminated\n").unwrap();
    let records = [
        record(
            root.path(),
            &coan,
            &["source", "-r", "-K", "-DFEATURE=1", "good.c"],
            "",
        ),
        record(
            root.path(),
            &coan,
            &["source", "-r", "-K", "-DFEATURE=1", "bad.c"],
            "",
        ),
    ];
    let cancelled = AtomicBool::new(false);
    let report = batch(&records, root.path(), &cancelled);
    // This distinguishes a single real external batch from run_each, which
    // would incorrectly certify good.c before observing bad.c's error.
    assert_eq!(report.outcomes.len(), 2);
    assert!(report.outcomes.iter().all(|(_, result)| result.is_err()));
    assert!(cancelled.load(Ordering::Acquire));
    assert_eq!(
        fs::read_to_string(root.path().join("good.c")).unwrap(),
        "kept\n"
    );
}

#[cfg(unix)]
#[test]
fn newly_introduced_aliases_and_directories_are_rejected_before_edits() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("safe.c"), CONDITIONAL).unwrap();
    fs::write(root.path().join("other.c"), CONDITIONAL).unwrap();
    let safe = record(
        root.path(),
        "coan",
        &["source", "-r", "-DFEATURE=1", "safe.c"],
        "",
    );
    let other = record(
        root.path(),
        "coan",
        &["source", "-r", "-DFEATURE=1", "other.c"],
        "",
    );
    assert!(!safe.spec.unknown_effects);
    fs::remove_file(root.path().join("other.c")).unwrap();
    symlink("safe.c", root.path().join("other.c")).unwrap();
    let report = batch(&[safe, other], root.path(), &AtomicBool::new(false));
    assert_eq!(report.outcomes.len(), 2);
    assert!(report.outcomes.iter().all(|(_, result)| result.is_err()));
    assert_eq!(
        fs::read_to_string(root.path().join("safe.c")).unwrap(),
        CONDITIONAL
    );

    fs::remove_file(root.path().join("other.c")).unwrap();
    fs::write(root.path().join("other.c"), CONDITIONAL).unwrap();
    let directory = record(
        root.path(),
        "coan",
        &["source", "-r", "-DFEATURE=1", "other.c"],
        "",
    );
    fs::remove_file(root.path().join("other.c")).unwrap();
    fs::create_dir(root.path().join("other.c")).unwrap();
    assert!(execute(&directory, root.path()).is_err());

    let generated = record(
        root.path(),
        "coan",
        &["source", "-r", "-DFEATURE=1", "generated.c"],
        "",
    );
    assert!(!generated.spec.unknown_effects);
    symlink("safe.c", root.path().join("generated.c")).unwrap();
    assert!(execute(&generated, root.path()).is_err());
    assert_eq!(
        fs::read_to_string(root.path().join("safe.c")).unwrap(),
        CONDITIONAL
    );

    let linked = record(
        root.path(),
        "coan",
        &["source", "-r", "-DFEATURE=1", "safe.c"],
        "",
    );
    fs::hard_link(root.path().join("safe.c"), root.path().join("alias.c")).unwrap();
    assert!(execute(&linked, root.path()).is_err());
    assert_eq!(
        fs::read_to_string(root.path().join("safe.c")).unwrap(),
        CONDITIONAL
    );
}

#[cfg(unix)]
#[test]
fn real_quotes_and_newlines_in_names_execute_separately_not_as_stdin_lists() {
    let Some(coan) = reference() else { return };
    let root = tempfile::tempdir().unwrap();
    let names = ["quoted\"name.c", "line\nbreak.c"];
    for name in names {
        fs::write(root.path().join(name), CONDITIONAL).unwrap();
    }
    let records: Vec<_> = names
        .iter()
        .map(|name| {
            record(
                root.path(),
                &coan,
                &["source", "-r", "-DFEATURE=1", name],
                "",
            )
        })
        .collect();
    let report = batch(&records, root.path(), &AtomicBool::new(false));
    assert_eq!(report.outcomes.len(), 2);
    assert!(report.outcomes.iter().all(|(_, result)| result.is_ok()));
    for name in names {
        assert_eq!(
            fs::read_to_string(root.path().join(name)).unwrap(),
            "kept\n"
        );
    }
}

#[test]
fn cancelled_batch_does_not_start_any_command() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("file.c"), CONDITIONAL).unwrap();
    let job = record(
        root.path(),
        "nonexistent-coan-executable",
        &["source", "-r", "-DFEATURE=1", "file.c"],
        "",
    );
    let report = batch(&[job], root.path(), &AtomicBool::new(true));
    assert!(report.outcomes.is_empty());
    assert_eq!(
        fs::read_to_string(root.path().join("file.c")).unwrap(),
        CONDITIONAL
    );
}

#[test]
fn known_missing_option_values_fail_the_whole_plan_before_prefix_execution() {
    let root = tempfile::tempdir().unwrap();
    let registry = Registry::default();
    for option in [
        "-D",
        "--define",
        "-U",
        "--undef",
        "--gag",
        "--discard",
        "--conflict",
        "--max-expansion",
        "--filter",
        "--file",
        "--backup",
        "--define=",
    ] {
        let text = format!("touch should-not-exist\ncoan source -r {option}\n");
        let error = rusty_make::parse_plan(&text, root.path(), &registry).unwrap_err();
        assert!(
            format!("{error:#}").contains("requires"),
            "{option}: {error:#}"
        );
        assert!(!root.path().join("should-not-exist").exists());
    }
}

#[test]
fn refinement_retains_intrinsic_paths_for_registry_snapshot_restoration() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("directory")).unwrap();
    let job = record(
        root.path(),
        "coan",
        &["source", "-r", "-DFEATURE=1", "directory"],
        "",
    );
    assert!(job.spec.unknown_effects);
    assert_eq!(job.spec.inputs, [PathBuf::from("directory")]);
    assert_eq!(job.spec.outputs, [PathBuf::from("directory")]);
    let snapshot = serde_json::from_value(serde_json::to_value(&job).unwrap()).unwrap();
    let restored = Registry::default().restore(snapshot).unwrap();
    assert!(restored.spec.unknown_effects);
    assert_eq!(restored.spec.inputs, job.spec.inputs);
    assert_eq!(restored.spec.outputs, job.spec.outputs);
}

#[test]
fn missing_generated_inputs_remain_conditionally_precise_and_persistable() {
    let root = tempfile::tempdir().unwrap();
    let job = record(
        root.path(),
        "coan",
        &["source", "-r", "-DFEATURE=1", "generated.c"],
        "",
    );
    assert_eq!(job.spec.unknown_effects, !cfg!(unix));
    assert_eq!(job.spec.inputs, [PathBuf::from("generated.c")]);
    assert_eq!(job.spec.outputs, [PathBuf::from("generated.c")]);
    let snapshot = serde_json::from_value(serde_json::to_value(&job).unwrap()).unwrap();
    let restored = Registry::default().restore(snapshot).unwrap();
    assert_eq!(restored.spec.unknown_effects, !cfg!(unix));
    #[cfg(unix)]
    assert!(
        restored
            .typed::<Coan>()
            .unwrap()
            .validate(&restored, root.path())
            .is_err()
    );
    let graph = rusty_make::parse_plan(
        "cp template.c generated.c\ncoan source -r -DFEATURE=1 generated.c\n",
        root.path(),
        &Registry::default(),
    )
    .unwrap();
    assert_eq!(graph.jobs[1].spec.unknown_effects, !cfg!(unix));
    assert!(
        graph
            .edges
            .iter()
            .any(|edge| edge.from == 0 && edge.to == 1)
    );
}

#[test]
fn real_generated_replacement_targets_execute_after_the_producer() {
    let Some(coan) = reference() else { return };
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("template.c"), CONDITIONAL).unwrap();
    let registry = Registry::default();
    let records = vec![
        registry
            .classify(source("cp template.c 'first generated.c'"))
            .unwrap(),
        registry
            .classify(source("cp template.c 'second generated.c'"))
            .unwrap(),
        record(
            root.path(),
            &coan,
            &["source", "-r", "-DFEATURE=1", "first generated.c"],
            "",
        ),
        record(
            root.path(),
            &coan,
            &["source", "-r", "-DFEATURE=1", "second generated.c"],
            "",
        ),
    ];
    assert!(
        records[2..]
            .iter()
            .all(|record| record.spec.unknown_effects == !cfg!(unix))
    );
    let graph = rusty_make::graph::BuildGraph::new(records, root.path()).unwrap();
    let report =
        rusty_make::scheduler::run(&graph, &rusty_make::scheduler::RunOptions::default()).unwrap();
    assert_eq!(report.executed, 4);
    for name in ["first generated.c", "second generated.c"] {
        assert_eq!(
            fs::read_to_string(root.path().join(name)).unwrap(),
            "kept\n"
        );
    }
}

#[test]
fn real_quiet_null_redirects_and_noop_fallback_preserve_coan_status() {
    let Some(coan) = reference() else { return };
    let root = tempfile::tempdir().unwrap();
    for redirect in [
        " > /dev/null",
        " 2> /dev/null",
        " > /dev/null 2> /dev/null || cd .",
    ] {
        fs::write(root.path().join("quiet.c"), CONDITIONAL).unwrap();
        let job = record(
            root.path(),
            &coan,
            &["source", "-r", "-DFEATURE=1", "quiet.c"],
            redirect,
        );
        #[cfg(unix)]
        assert!(!job.spec.unknown_effects);
        assert_eq!(job.spec.inputs, [PathBuf::from("quiet.c")]);
        assert_eq!(job.spec.outputs, [PathBuf::from("quiet.c")]);
        execute(&job, root.path()).unwrap();
        assert_eq!(
            fs::read_to_string(root.path().join("quiet.c")).unwrap(),
            "kept\n"
        );
    }
    let bad = record(
        root.path(),
        &coan,
        &["source", "--not-a-real-coan-option"],
        " > /dev/null 2> /dev/null || cd .",
    );
    assert!(bad.shell.as_ref().unwrap().noop_fallback);
    assert!(
        execute(&bad, root.path()).is_err(),
        "cd . must not mask a coan error"
    );
    assert!(!root.path().join("NUL").exists());
}

#[cfg(unix)]
#[test]
fn real_quiet_compatible_jobs_batch_and_aggregate_failures() {
    let Some(coan) = reference() else { return };
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("good.c"), CONDITIONAL).unwrap();
    fs::write(root.path().join("bad.c"), "#ifdef FEATURE\nunterminated\n").unwrap();
    let records = [
        record(
            root.path(),
            &coan,
            &["source", "-r", "-K", "-DFEATURE=1", "good.c"],
            " > /dev/null 2> /dev/null || cd .",
        ),
        record(
            root.path(),
            &coan,
            &["source", "-r", "-K", "-DFEATURE=1", "bad.c"],
            " > /dev/null 2> /dev/null || cd .",
        ),
    ];
    assert!(records.iter().all(|record| !record.spec.unknown_effects));
    let report = batch(&records, root.path(), &AtomicBool::new(false));
    assert_eq!(report.outcomes.len(), 2);
    assert!(report.outcomes.iter().all(|(_, result)| result.is_err()));
    assert_eq!(
        fs::read_to_string(root.path().join("good.c")).unwrap(),
        "kept\n"
    );
}

#[cfg(unix)]
#[test]
fn real_different_quiet_settings_do_not_share_aggregate_status() {
    let Some(coan) = reference() else { return };
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("good.c"), CONDITIONAL).unwrap();
    fs::write(root.path().join("bad.c"), "#ifdef FEATURE\nunterminated\n").unwrap();
    let records = [
        record(
            root.path(),
            &coan,
            &["source", "-r", "-K", "-DFEATURE=1", "good.c"],
            "",
        ),
        record(
            root.path(),
            &coan,
            &["source", "-r", "-K", "-DFEATURE=1", "bad.c"],
            " 2> /dev/null",
        ),
    ];
    let report = batch(&records, root.path(), &AtomicBool::new(false));
    assert_eq!(report.outcomes.len(), 2);
    assert!(
        report
            .outcomes
            .iter()
            .find(|(id, _)| *id == 0)
            .unwrap()
            .1
            .is_ok()
    );
    assert!(
        report
            .outcomes
            .iter()
            .find(|(id, _)| *id == 1)
            .unwrap()
            .1
            .is_err()
    );
}
