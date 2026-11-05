use rusty_make::{
    graph::{BuildGraph, Edge},
    input::SourceCommand,
    job::{ExecutionStrategy, Job, JobSpec, Operation},
};
use std::path::PathBuf;
fn job(inputs: &[&str], outputs: &[&str], unknown: bool) -> Job {
    Job {
        source: SourceCommand {
            line: 1,
            text: "test effect".into(),
            target: None,
        },
        shell: None,
        strategy: ExecutionStrategy::External,
        spec: JobSpec {
            kind: "test".into(),
            operation: Operation::External,
            inputs: inputs.iter().map(PathBuf::from).collect(),
            outputs: outputs.iter().map(PathBuf::from).collect(),
            unknown_effects: unknown,
            cacheable: false,
            parsed: serde_json::json!({}),
        },
    }
}
fn graph(jobs: Vec<Job>) -> BuildGraph {
    BuildGraph::new(jobs, &std::env::current_dir().unwrap()).unwrap()
}
#[test]
fn independent_jobs_and_branching_multiple_io() {
    let g = graph(vec![
        job(&["a"], &["x", "y"], false),
        job(&["b"], &["z"], false),
        job(&["x", "z"], &["q"], false),
        job(&["y"], &["r"], false),
    ]);
    assert_eq!(g.waves, vec![vec![0, 1], vec![2, 3]]);
    assert_eq!(
        g.edges.iter().map(|e| (e.from, e.to)).collect::<Vec<_>>(),
        vec![(0, 2), (1, 2), (0, 3)]
    );
}
#[test]
fn versions_preserve_in_place_reads_and_write_after_read() {
    let g = graph(vec![
        job(&["a"], &["b"], false),
        job(&["b"], &["c"], false),
        job(&["b"], &["b"], false),
        job(&["b"], &["d"], false),
    ]);
    assert_eq!(g.reads[1][0].version, 1);
    assert_eq!(g.reads[2][0].version, 1);
    assert_eq!(g.writes[2][0].version, 2);
    assert_eq!(g.reads[3][0].version, 2);
    assert!(
        g.edges
            .iter()
            .any(|e| e.from == 1 && e.to == 2 && e.reason == "write after read")
    );
    assert_eq!(g.waves, vec![vec![0], vec![1], vec![2], vec![3]]);
}
#[test]
fn directory_prerequisites_and_duplicate_writers_are_ordered() {
    let g = graph(vec![
        job(&[], &["build"], false),
        job(&["a"], &["build/x"], false),
        job(&["b"], &["build/x"], false),
    ]);
    assert_eq!(g.waves, vec![vec![0], vec![1], vec![2]]);
}
#[test]
fn unknown_effects_are_global_barriers() {
    let g = graph(vec![
        job(&[], &["a"], false),
        job(&[], &["b"], false),
        job(&[], &[], true),
        job(&[], &["c"], false),
        job(&[], &["d"], false),
    ]);
    assert_eq!(g.waves, vec![vec![0, 1], vec![2], vec![3, 4]]);
}
#[test]
fn cycle_errors_identify_commands() {
    let mut g = graph(vec![job(&[], &["a"], false), job(&["a"], &["b"], false)]);
    g.edges.push(Edge {
        from: 1,
        to: 0,
        reason: "test cycle".into(),
    });
    let error = g.topological_waves().unwrap_err().to_string();
    assert!(error.contains("cycle") && error.contains("test effect"));
}
#[test]
fn relative_absolute_paths_refer_to_same_artifact() {
    let cwd = std::env::current_dir().unwrap();
    let absolute = cwd.join("build/path");
    let g = graph(vec![
        job(&[], &["./build/path"], false),
        job(&[absolute.to_str().unwrap()], &["x"], false),
    ]);
    assert_eq!(g.waves, vec![vec![0], vec![1]]);
}
#[cfg(unix)]
#[test]
fn symlink_parent_dotdot_resolves_in_filesystem_order() {
    use rusty_make::graph::normalize;
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("actual/child")).unwrap();
    std::os::unix::fs::symlink(root.path().join("actual/child"), root.path().join("link")).unwrap();
    assert_eq!(
        normalize(root.path(), std::path::Path::new("link/../missing")),
        root.path().join("actual/missing")
    );
}

#[test]
fn mkdir_parents_orders_sibling_output_using_implicit_parent() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("source"), "payload").unwrap();
    let graph = rusty_make::parse_plan(
        "mkdir -p build/nested\ncp source build/sibling\n",
        root.path(),
        &rusty_make::registry::Registry::default(),
    )
    .unwrap();
    assert_eq!(graph.waves, vec![vec![0], vec![1]]);
    rusty_make::scheduler::run(
        &graph,
        &rusty_make::scheduler::RunOptions {
            jobs: 2,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(root.path().join("build/sibling")).unwrap(),
        "payload"
    );
}
