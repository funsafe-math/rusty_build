use rusty_make::{
    parse_plan,
    registry::Registry,
    rewrite::{CopyChain, GraphRewrite},
    scheduler::{RunOptions, run},
};
use std::{fs, path::Path};
fn hash(path: &Path) -> String {
    blake3::hash(&fs::read(path).unwrap()).to_hex().to_string()
}
#[test]
fn independent_jobs_batch_and_dependent_jobs_wait() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("a"), "A").unwrap();
    fs::write(p.join("b"), "B").unwrap();
    let graph = parse_plan("cp a x\ncp b y\ncp x z\n", p, &Registry::default()).unwrap();
    assert_eq!(graph.waves, vec![vec![0, 1], vec![2]]);
    let report = run(
        &graph,
        &RunOptions {
            jobs: 4,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(report.executed, 3);
    assert_eq!(report.batches, 2);
    assert_eq!(fs::read_to_string(p.join("z")).unwrap(), "A");
    assert_eq!(fs::read_to_string(p.join("y")).unwrap(), "B");
}
#[test]
fn different_types_remain_distinct_batches() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("a"), "A").unwrap();
    let graph = parse_plan("cp a x\nmkdir -p independent\n", p, &Registry::default()).unwrap();
    assert_eq!(graph.waves, vec![vec![0, 1]]);
    let report = run(
        &graph,
        &RunOptions {
            jobs: 2,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(report.batches, 2);
    assert!(p.join("independent").is_dir());
    assert_eq!(fs::read(p.join("x")).unwrap(), b"A");
}
#[test]
fn failure_prevents_dependent_work() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    let graph = parse_plan(
        "cp missing x\ncp x should-not-exist\n",
        p,
        &Registry::default(),
    )
    .unwrap();
    assert!(
        run(
            &graph,
            &RunOptions {
                jobs: 4,
                ..Default::default()
            }
        )
        .is_err()
    );
    assert!(!p.join("should-not-exist").exists());
}
#[test]
fn rewrite_preserves_intermediate_final_artifacts_and_later_consumers() {
    let original = tempfile::tempdir().unwrap();
    let optimized = tempfile::tempdir().unwrap();
    let commands = "cp source intermediate\ncp intermediate final\ncp intermediate other\n";
    for p in [original.path(), optimized.path()] {
        fs::write(p.join("source"), "shared input\n").unwrap();
    }
    let reference = parse_plan(commands, original.path(), &Registry::default()).unwrap();
    run(&reference, &RunOptions::default()).unwrap();
    let mut graph = parse_plan(commands, optimized.path(), &Registry::default()).unwrap();
    let stats = CopyChain.apply(&mut graph).unwrap();
    assert_eq!(stats.transformations, 1);
    assert_eq!(graph.provenance[0].len(), 2);
    run(
        &graph,
        &RunOptions {
            jobs: 4,
            ..Default::default()
        },
    )
    .unwrap();
    for name in ["source", "intermediate", "final", "other"] {
        assert_eq!(
            hash(&original.path().join(name)),
            hash(&optimized.path().join(name))
        );
    }
    let second = run(&graph, &RunOptions::default()).unwrap();
    assert_eq!(second.skipped, 2);
}
#[test]
fn rewrite_refuses_overwriting_original_source() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("a"), "A").unwrap();
    let mut graph = parse_plan("cp a b\ncp b a\n", root.path(), &Registry::default()).unwrap();
    assert_eq!(CopyChain.apply(&mut graph).unwrap().transformations, 0);
}
#[test]
fn copy_chain_existing_destination_preserves_sequential_behavior() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    for (name, value) in [("a", "new"), ("b", "old-b"), ("c", "old-c")] {
        fs::write(p.join(name), value).unwrap();
    }
    let mut graph = parse_plan("cp a b\ncp b c\n", p, &Registry::default()).unwrap();
    CopyChain.apply(&mut graph).unwrap();
    run(&graph, &RunOptions::default()).unwrap();
    assert_eq!(fs::read_to_string(p.join("b")).unwrap(), "new");
    assert_eq!(fs::read_to_string(p.join("c")).unwrap(), "new");
}
#[test]
fn invalid_plan_does_not_execute_valid_prefix() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("a"), "A").unwrap();
    fs::write(p.join("commands.txt"), "cp a b\ncp --unsupported b c\n").unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_rusty_make"))
        .current_dir(p)
        .args(["--commands", "commands.txt"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!p.join("b").exists());
}
