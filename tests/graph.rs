use rusty_make::{
    graph::{BuildGraph, Edge},
    input::SourceCommand,
    job::{ExecutionContext, Job, JobRecord},
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize)]
struct TestEffects {
    inputs: Vec<PathBuf>,
    outputs: Vec<PathBuf>,
    unknown: bool,
}
impl Job for TestEffects {
    const TYPE: &'static str = "test.effects";
    fn parse(_: &rusty_make::shell::ShellCommand) -> anyhow::Result<Self> {
        anyhow::bail!("graph-effect fixtures are constructed directly")
    }
    fn inputs(&self) -> impl Iterator<Item = &Path> {
        self.inputs.iter().map(PathBuf::as_path)
    }
    fn outputs(&self) -> impl Iterator<Item = &Path> {
        self.outputs.iter().map(PathBuf::as_path)
    }
    fn unknown_effects(&self) -> bool {
        self.unknown
    }
    fn execute(&self, ctx: &ExecutionContext<'_>) -> anyhow::Result<()> {
        rusty_make::commands::external::execute_original(ctx)
    }
}
fn job(inputs: &[&str], outputs: &[&str], unknown: bool) -> JobRecord {
    JobRecord::new(
        SourceCommand {
            line: 1,
            text: "test effect".into(),
            target: None,
        },
        None,
        TestEffects {
            inputs: inputs.iter().map(PathBuf::from).collect(),
            outputs: outputs.iter().map(PathBuf::from).collect(),
            unknown,
        },
    )
    .unwrap()
}
fn graph(jobs: Vec<JobRecord>) -> BuildGraph {
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

fn reachability(graph: &BuildGraph) -> Vec<Vec<bool>> {
    let mut children = vec![Vec::new(); graph.jobs.len()];
    for edge in &graph.edges {
        children[edge.from].push(edge.to);
    }
    (0..graph.jobs.len())
        .map(|start| {
            let mut reached = vec![false; graph.jobs.len()];
            let mut pending = children[start].clone();
            while let Some(node) = pending.pop() {
                if !reached[node] {
                    reached[node] = true;
                    pending.extend_from_slice(&children[node]);
                }
            }
            reached
        })
        .collect()
}

#[test]
fn indexed_hazards_and_frontiers_match_dense_ordering_and_waves() {
    let root = tempfile::tempdir().unwrap();
    let paths = ["a", "a/one", "a/two", "b", "b/one", "c", "c/deep/file", "d"];
    for seed in 1..=6u64 {
        let mut state = seed;
        let mut next = || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            (state >> 32) as usize
        };
        let jobs = (0..72)
            .map(|_| {
                let inputs = [paths[next() % paths.len()], paths[next() % paths.len()]];
                let outputs = [paths[next() % paths.len()], paths[next() % paths.len()]];
                job(
                    &inputs[..next() % 3],
                    &outputs[..next() % 3],
                    next() % 5 == 0,
                )
            })
            .collect();
        let actual = BuildGraph::new(jobs, root.path()).unwrap();
        let mut dense = actual.clone();
        dense.edges.clear();
        for to in 0..actual.jobs.len() {
            for from in 0..to {
                let overlap =
                    |left: &[rusty_make::graph::ArtifactId],
                     right: &[rusty_make::graph::ArtifactId]| {
                        left.iter().any(|a| {
                            right
                                .iter()
                                .any(|b| a.path.starts_with(&b.path) || b.path.starts_with(&a.path))
                        })
                    };
                let hazard = if overlap(&actual.writes[from], &actual.reads[to]) {
                    Some("read after write / directory prerequisite")
                } else if overlap(&actual.writes[from], &actual.writes[to]) {
                    Some("write after write")
                } else if overlap(&actual.reads[from], &actual.writes[to]) {
                    Some("write after read")
                } else {
                    None
                };
                if let Some(reason) = hazard {
                    // Actual hazards stay directly inspectable, not just
                    // implied by otherwise conservative barrier reachability.
                    assert!(actual.edges.iter().any(|edge| {
                        edge.from == from && edge.to == to && edge.reason == reason
                    }));
                }
                if let Some(reason) = hazard.or_else(|| {
                    (actual.jobs[from].spec.unknown_effects || actual.jobs[to].spec.unknown_effects)
                        .then_some("unknown-effect or filesystem-alias ordering barrier")
                }) {
                    dense.edges.push(Edge {
                        from,
                        to,
                        reason: reason.into(),
                    });
                }
            }
        }
        assert_eq!(reachability(&actual), reachability(&dense), "seed {seed}");
        assert_eq!(
            actual.waves,
            dense.topological_waves().unwrap(),
            "seed {seed}"
        );
        assert!(actual.edges.iter().all(|edge| edge.from < edge.to));
    }
}

#[test]
fn independent_large_paths_do_not_create_edges() {
    let root = tempfile::tempdir().unwrap();
    let jobs = (0..4096)
        .map(|i| {
            job(
                &[&format!("sources/{i}.c")],
                &[&format!("objects/{i}.o")],
                false,
            )
        })
        .collect();
    let graph = BuildGraph::new(jobs, root.path()).unwrap();
    assert!(graph.edges.is_empty());
    assert_eq!(graph.waves.len(), 1);
    assert_eq!(graph.waves[0], (0..4096).collect::<Vec<_>>());
}

#[test]
fn large_opaque_and_compile_barriers_have_linear_edge_counts() {
    let root = tempfile::tempdir().unwrap();
    let count = 4096;
    let jobs = (0..count)
        .map(|i| {
            if i % 2 == 0 {
                job(&[], &[], true)
            } else {
                // Compiler metadata can contain paths while remaining an
                // unknown-effect barrier because of hidden include reads.
                job(
                    &[&format!("source/{i}.c")],
                    &[&format!("object/{i}.o")],
                    true,
                )
            }
        })
        .collect();
    let graph = BuildGraph::new(jobs, root.path()).unwrap();
    assert!(graph.edges.len() <= count * 2);
    assert_eq!(graph.waves, (0..count).map(|i| vec![i]).collect::<Vec<_>>());
}

#[test]
fn barriers_join_dependency_frontiers_not_every_earlier_job() {
    let root = tempfile::tempdir().unwrap();
    let groups = 32;
    let width = 64;
    let mut jobs = Vec::new();
    let mut expected_waves = Vec::new();
    for group in 0..groups {
        let start = jobs.len();
        for member in 0..width {
            jobs.push(job(&[], &[&format!("output/{group}/{member}")], false));
        }
        expected_waves.push((start..jobs.len()).collect::<Vec<_>>());
        expected_waves.push(vec![jobs.len()]);
        jobs.push(job(&[], &[], true));
    }
    let graph = BuildGraph::new(jobs, root.path()).unwrap();
    assert!(graph.edges.len() <= graph.jobs.len() * 2);
    assert_eq!(graph.waves, expected_waves);
    let reached = reachability(&graph);
    for (from, successors) in reached.iter().enumerate() {
        for (to, &reachable) in successors.iter().enumerate() {
            let from_group = from / (width + 1);
            let to_group = to / (width + 1);
            let expected = from_group < to_group
                || (from_group == to_group && from < to && to % (width + 1) == width);
            assert_eq!(reachable, expected, "{from} -> {to}");
        }
    }
}

#[cfg(unix)]
#[test]
fn rebuild_refreshes_symlink_normalization_and_hardlink_metadata() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    std::fs::write(p.join("a"), "").unwrap();
    std::fs::write(p.join("b"), "").unwrap();
    std::os::unix::fs::symlink("a", p.join("link")).unwrap();
    let mut graph = BuildGraph::new(
        vec![
            job(&[], &["a"], false),
            job(&["link", "link"], &["other"], false),
        ],
        p,
    )
    .unwrap();
    assert_eq!(graph.waves, vec![vec![0], vec![1]]);
    std::fs::remove_file(p.join("link")).unwrap();
    std::os::unix::fs::symlink("b", p.join("link")).unwrap();
    graph.rebuild().unwrap();
    assert_eq!(graph.waves, vec![vec![0, 1]]);
    std::fs::hard_link(p.join("a"), p.join("alias")).unwrap();
    graph.rebuild().unwrap();
    assert_eq!(graph.waves, vec![vec![0], vec![1]]);
    assert!(reachability(&graph)[0][1]);
    std::fs::remove_file(p.join("alias")).unwrap();
    graph.rebuild().unwrap();
    assert_eq!(graph.waves, vec![vec![0, 1]]);
}

#[test]
fn backward_required_headers_keep_final_versions_and_intervening_cycles() {
    use rusty_make::graph::RequiredDependency;
    let root = tempfile::tempdir().unwrap();
    let make_jobs = |intervening_unknown| {
        let mut jobs = vec![
            job(&["generated.h"], &["main.o"], true),
            job(&[], &["independent"], intervening_unknown),
            job(&["template"], &["generated.h"], false),
        ];
        for (index, job) in jobs.iter_mut().enumerate() {
            job.source.line = index + 1;
        }
        jobs
    };
    let dependency = RequiredDependency {
        producer_line: 3,
        consumer_line: 1,
        artifact: "generated.h".into(),
        reason: "selected generated header".into(),
    };
    let mut safe = BuildGraph::new(make_jobs(false), root.path()).unwrap();
    safe.required_dependencies.push(dependency.clone());
    safe.rebuild().unwrap();
    assert_eq!(safe.waves, vec![vec![2], vec![0], vec![1]]);
    assert_eq!(safe.reads[0][0].version, safe.writes[2][0].version);
    assert!(reachability(&safe)[2][0]);
    assert!(!reachability(&safe)[0][2]);
    let mut unsafe_graph = BuildGraph::new(make_jobs(true), root.path()).unwrap();
    unsafe_graph.required_dependencies.push(dependency);
    let error = unsafe_graph.rebuild().unwrap_err().to_string();
    assert!(error.contains("dependency cycle"));
    assert!(error.contains("line 1") && error.contains("line 3"));
}
