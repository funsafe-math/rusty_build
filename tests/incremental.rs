use rusty_make::{
    parse_plan,
    registry::Registry,
    scheduler::{RunOptions, run},
};
use std::{fs, path::Path};
fn execute(root: &Path, text: &str, rebuild: bool) -> rusty_make::scheduler::RunReport {
    let graph = parse_plan(text, root, &Registry::default()).unwrap();
    run(
        &graph,
        &RunOptions {
            jobs: 4,
            rebuild_all: rebuild,
            ..Default::default()
        },
    )
    .unwrap()
}
#[test]
fn clean_second_input_and_output_changes() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("a"), "first").unwrap();
    let commands = "cp a b\ncp b c\n";
    assert_eq!(execute(p, commands, false).executed, 2);
    assert_eq!(execute(p, commands, false).skipped, 2);
    fs::write(p.join("a"), "second").unwrap();
    assert_eq!(execute(p, commands, false).executed, 2);
    assert_eq!(fs::read_to_string(p.join("c")).unwrap(), "second");
    fs::remove_file(p.join("c")).unwrap();
    let report = execute(p, commands, false);
    assert_eq!((report.executed, report.skipped), (1, 1));
    fs::write(p.join("b"), "tampered").unwrap();
    let report = execute(p, commands, false);
    assert_eq!((report.executed, report.skipped), (1, 1));
    assert_eq!(fs::read_to_string(p.join("b")).unwrap(), "second");
    fs::write(p.join("c"), "tampered-final").unwrap();
    assert_eq!(execute(p, commands, false).executed, 1);
    assert_eq!(fs::read_to_string(p.join("c")).unwrap(), "second");
}
#[test]
fn changed_arguments_but_identical_upstream_output_do_not_rebuild_downstream() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("a"), "same").unwrap();
    execute(p, "cp a b\ncp b c\n", false);
    let report = execute(p, "cp ./a b\ncp b c\n", false);
    assert_eq!((report.executed, report.skipped), (1, 1));
    assert_eq!(execute(p, "cp ./a b\ncp b c\n", true).executed, 2);
}
#[test]
fn failed_job_invalidates_prior_success_record() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("a"), "source").unwrap();
    execute(p, "cp a b\n", false);
    fs::remove_file(p.join("a")).unwrap();
    let graph = parse_plan("cp a b\n", p, &Registry::default()).unwrap();
    assert!(run(&graph, &RunOptions::default()).is_err());
    let cache: serde_json::Value =
        serde_json::from_slice(&fs::read(p.join(".rusty_make/cache.json")).unwrap()).unwrap();
    assert!(cache["records"].as_object().unwrap().is_empty());
    fs::write(p.join("a"), "source").unwrap();
    assert_eq!(execute(p, "cp a b\n", false).executed, 1);
}
#[test]
fn unknown_dependencies_disable_unsafe_cache_hits() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("a"), "source").unwrap();
    let text = "true\ncp a b\n";
    assert_eq!(execute(p, text, false).executed, 2);
    assert_eq!(execute(p, text, false).executed, 2);
}
#[test]
fn dry_run_has_no_filesystem_side_effects() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("a"), "source").unwrap();
    let graph = parse_plan("cp a b\n", p, &Registry::default()).unwrap();
    let report = run(
        &graph,
        &RunOptions {
            dry_run: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(report.executed, 0);
    assert_eq!(report.planned, 1);
    assert!(!p.join("b").exists());
    assert!(!p.join(".rusty_make").exists());
}
#[test]
fn compiler_never_skips_unknown_header_dependencies() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    if std::process::Command::new("cc")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    fs::write(p.join("a.h"), "#define VALUE 1\n").unwrap();
    fs::write(
        p.join("a.c"),
        "#include \"a.h\"\nint value(void){return VALUE;}\n",
    )
    .unwrap();
    execute(p, "cc -c a.c -o a.o\n", false);
    assert_eq!(execute(p, "cc -c a.c -o a.o\n", false).executed, 1);
    let before = fs::read(p.join("a.o")).unwrap();
    fs::write(p.join("a.h"), "#define VALUE 2\n").unwrap();
    execute(p, "cc -c a.c -o a.o\n", false);
    assert_ne!(before, fs::read(p.join("a.o")).unwrap());
}
