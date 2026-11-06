use rusty_make::{
    graph::{Dag, Edge, create_graph},
    job::{FileAccess, Job, JobData, Producer},
    rewrite::optimize_graph,
};
use std::{cell::RefCell, rc::Rc};

#[derive(Debug)]
struct Effects {
    data: JobData,
}
impl Job for Effects {
    fn data(&self) -> &JobData {
        &self.data
    }
    fn data_mut(&mut self) -> &mut JobData {
        &mut self.data
    }
    fn execute(&self) -> anyhow::Result<()> {
        Ok(())
    }
}
fn job(accesses: Vec<FileAccess>) -> Box<dyn Job> {
    Box::new(Effects {
        data: JobData::new(accesses),
    })
}
fn unknown() -> Box<dyn Job> {
    let mut item = job(Vec::new());
    item.data_mut().unknown_effects = true;
    item
}
fn bind(mut consumer: Box<dyn Job>, input: &str, producer: usize) -> Box<dyn Job> {
    consumer.data_mut().producers.push(Producer {
        input: input.into(),
        job: producer,
    });
    consumer
}
fn reaches(dag: &Dag, from: usize, to: usize) -> bool {
    let mut pending = vec![from];
    let mut visited = vec![false; dag.jobs.len()];
    while let Some(node) = pending.pop() {
        if visited[node] {
            continue;
        }
        visited[node] = true;
        for edge in dag.edges.iter().filter(|edge| edge.from == node) {
            if edge.to == to {
                return true;
            }
            pending.push(edge.to);
        }
    }
    false
}

#[test]
fn independent_jobs_and_branching_multiple_accesses() {
    let dag = create_graph(vec![
        job(vec![
            FileAccess::read("a"),
            FileAccess::write("x"),
            FileAccess::write("y"),
        ]),
        job(vec![FileAccess::read("b"), FileAccess::write("z")]),
        job(vec![
            FileAccess::read("x"),
            FileAccess::read("z"),
            FileAccess::write("q"),
        ]),
        job(vec![FileAccess::read("y"), FileAccess::write("r")]),
    ])
    .unwrap();
    assert_eq!(dag.waves, vec![vec![0, 1], vec![2, 3]]);
    assert_eq!(dag.unproduced, vec![vec!["a"], vec!["b"], vec![], vec![]]);
}

#[test]
fn raw_war_waw_and_read_modify_write_preserve_values() {
    let dag = create_graph(vec![
        job(vec![FileAccess::write("value")]),
        job(vec![FileAccess::read("value")]),
        job(vec![FileAccess::read_write("value")]),
        job(vec![FileAccess::read("value")]),
        job(vec![FileAccess::write("value")]),
    ])
    .unwrap();
    assert_eq!(dag.waves, vec![vec![0], vec![1], vec![2], vec![3], vec![4]]);
    assert!(dag.unproduced.iter().all(Vec::is_empty));
    assert!(dag.edges.iter().all(|edge| edge.from != edge.to));
}

#[test]
fn read_before_first_write_and_initial_read_write_are_unproduced() {
    let dag = create_graph(vec![
        job(vec![FileAccess::read("value"), FileAccess::read("value")]),
        job(vec![FileAccess::read_write("value")]),
        job(vec![FileAccess::read_write("initial")]),
    ])
    .unwrap();
    assert_eq!(
        dag.unproduced,
        vec![vec!["value"], vec!["value"], vec!["initial"]]
    );
    assert!(reaches(&dag, 0, 1));
}

#[test]
fn explicit_later_producer_selects_its_value_and_guards_next_overwrite() {
    let dag = create_graph(vec![
        bind(job(vec![FileAccess::read("header")]), "header", 2),
        job(vec![FileAccess::write("header")]),
        job(vec![FileAccess::write("header")]),
        job(vec![FileAccess::write("header")]),
    ])
    .unwrap();
    assert_eq!(dag.waves, vec![vec![1], vec![2], vec![0], vec![3]]);
    assert!(dag.unproduced[0].is_empty());
}

#[test]
fn explicit_preceding_producer_reads_before_a_later_overwrite() {
    let dag = create_graph(vec![
        job(vec![FileAccess::write("value")]),
        job(vec![FileAccess::write("value")]),
        bind(job(vec![FileAccess::read("value")]), "value", 0),
    ])
    .unwrap();
    assert_eq!(dag.waves, vec![vec![0], vec![2], vec![1]]);
}

#[test]
fn directory_hazards_include_ancestors_descendants_but_not_siblings() {
    let dag = create_graph(vec![
        job(vec![FileAccess::write("build")]),
        job(vec![FileAccess::write("build/a")]),
        job(vec![FileAccess::write("build/b")]),
        job(vec![FileAccess::read("build")]),
        job(vec![FileAccess::write("build/a/nested")]),
        job(vec![FileAccess::write("build-other/a")]),
    ])
    .unwrap();
    assert!(reaches(&dag, 0, 1) && reaches(&dag, 0, 2));
    assert!(!reaches(&dag, 1, 2) && !reaches(&dag, 2, 1));
    assert!(reaches(&dag, 1, 3) && reaches(&dag, 2, 3));
    assert!(reaches(&dag, 3, 4));
    assert!(!reaches(&dag, 0, 5));
}

#[test]
fn absolute_root_and_trailing_directory_slashes_preserve_hazards() {
    let dag = create_graph(vec![
        job(vec![FileAccess::write("/")]),
        job(vec![FileAccess::read("/absolute/file")]),
        job(vec![FileAccess::write("relative/")]),
        job(vec![FileAccess::read("relative/file")]),
    ])
    .unwrap();
    assert!(reaches(&dag, 0, 1));
    assert!(reaches(&dag, 2, 3));
    assert!(!reaches(&dag, 0, 3));
}

#[test]
fn ancestor_writer_can_bind_a_descendant_input() {
    let dag = create_graph(vec![
        bind(
            job(vec![FileAccess::read("generated/a.h")]),
            "generated/a.h",
            1,
        ),
        job(vec![FileAccess::write("generated")]),
    ])
    .unwrap();
    assert_eq!(dag.waves, vec![vec![1], vec![0]]);
}

#[test]
fn ancestor_writes_do_not_supply_missing_descendant_inputs() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("generated").to_str().unwrap().to_owned();
    let missing = format!("{directory}/absent.h");
    let dag = create_graph(vec![
        job(vec![FileAccess::write(directory)]),
        job(vec![FileAccess::read(missing)]),
    ])
    .unwrap();
    assert!(reaches(&dag, 0, 1));
    assert!(dag.check_inputs().is_err());
}

#[test]
fn reversed_unknown_producers_preserve_unrelated_barriers() {
    let mut consumer = bind(unknown(), "x", 1);
    consumer.data_mut().accesses.push(FileAccess::read("x"));
    let mut producer = unknown();
    producer.data_mut().accesses.push(FileAccess::write("x"));
    let dag = create_graph(vec![consumer, producer, unknown()]).unwrap();
    assert!(reaches(&dag, 1, 0));
    assert!(reaches(&dag, 0, 2));
    let mut consumer = bind(bind(unknown(), "x", 1), "z", 3);
    consumer
        .data_mut()
        .accesses
        .extend([FileAccess::read("x"), FileAccess::read("z")]);
    let mut producer = unknown();
    producer.data_mut().accesses.push(FileAccess::write("x"));
    assert!(
        create_graph(vec![
            consumer,
            producer,
            unknown(),
            job(vec![FileAccess::write("z")]),
        ])
        .unwrap_err()
        .to_string()
        .contains("cycle")
    );
}

#[test]
fn unknown_effect_barriers_are_sparse_and_keep_segment_jobs_independent() {
    let mut jobs: Vec<_> = (0..500)
        .map(|i| job(vec![FileAccess::write(format!("left/{i}"))]))
        .collect();
    jobs.push(unknown());
    jobs.extend((0..499).map(|i| job(vec![FileAccess::write(format!("right/{i}"))])));
    let dag = create_graph(jobs).unwrap();
    assert_eq!(
        dag.waves.iter().map(Vec::len).collect::<Vec<_>>(),
        vec![500, 1, 499]
    );
    assert_eq!(dag.edges.len(), 999);
    let all_barriers = create_graph((0..1000).map(|_| unknown()).collect()).unwrap();
    assert_eq!(all_barriers.edges.len(), 999);
}

#[test]
fn unknown_barrier_respects_producer_direction_but_not_intervening_cycles() {
    let mut consumer = bind(job(vec![FileAccess::read("header")]), "header", 1);
    consumer.data_mut().unknown_effects = true;
    let dag = create_graph(vec![
        consumer,
        job(vec![FileAccess::write("header")]),
        unknown(),
    ])
    .unwrap();
    assert_eq!(dag.waves, vec![vec![1], vec![0], vec![2]]);
    let mut consumer = bind(job(vec![FileAccess::read("header")]), "header", 2);
    consumer.data_mut().unknown_effects = true;
    let error = create_graph(vec![
        consumer,
        unknown(),
        job(vec![FileAccess::write("header")]),
    ])
    .unwrap_err();
    assert!(error.to_string().contains("cycle"));
}

#[test]
fn explicit_bindings_reject_cycles_and_invalid_producers() {
    let cycle = create_graph(vec![
        bind(
            job(vec![FileAccess::read("b"), FileAccess::write("a")]),
            "b",
            1,
        ),
        job(vec![FileAccess::read("a"), FileAccess::write("b")]),
    ])
    .unwrap_err();
    assert!(cycle.to_string().contains("cycle"));
    assert!(create_graph(vec![bind(job(vec![FileAccess::read("x")]), "x", 8)]).is_err());
    assert!(
        create_graph(vec![
            bind(job(vec![FileAccess::read("x")]), "x", 1),
            job(vec![FileAccess::write("other")])
        ])
        .is_err()
    );
    assert!(
        create_graph(vec![
            bind(job(vec![]), "x", 1),
            job(vec![FileAccess::write("x")])
        ])
        .is_err()
    );
}

#[test]
fn graph_construction_never_checks_files_but_check_inputs_does() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("absent").to_str().unwrap().to_owned();
    let dag = create_graph(vec![job(vec![FileAccess::read_write(missing.clone())])]).unwrap();
    assert!(dag.check_inputs().is_err());
    let dag = create_graph(vec![
        bind(job(vec![FileAccess::read(missing.clone())]), &missing, 1),
        job(vec![FileAccess::write(missing)]),
    ])
    .unwrap();
    dag.check_inputs().unwrap();
}

#[derive(Debug)]
struct InputObserver {
    data: JobData,
    calls: Rc<RefCell<Vec<Vec<String>>>>,
}
impl Job for InputObserver {
    fn data(&self) -> &JobData {
        &self.data
    }
    fn data_mut(&mut self) -> &mut JobData {
        &mut self.data
    }
    fn execute(&self) -> anyhow::Result<()> {
        Ok(())
    }
    fn check_inputs(&self, inputs: &[&str]) -> anyhow::Result<()> {
        self.calls
            .borrow_mut()
            .push(inputs.iter().map(|input| (*input).to_owned()).collect());
        Ok(())
    }
}
#[test]
fn input_checks_are_dispatched_to_the_owning_job() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let dag = create_graph(vec![Box::new(InputObserver {
        data: JobData::new(vec![
            FileAccess::read("initial"),
            FileAccess::write("output"),
        ]),
        calls: calls.clone(),
    })])
    .unwrap();
    assert!(calls.borrow().is_empty());
    dag.check_inputs().unwrap();
    assert_eq!(*calls.borrow(), vec![vec!["initial".to_owned()]]);
}

fn overlap(a: &str, b: &str) -> bool {
    a == b
        || a.strip_prefix(b).is_some_and(|tail| tail.starts_with('/'))
        || b.strip_prefix(a).is_some_and(|tail| tail.starts_with('/'))
}

#[test]
fn indexed_graph_matches_dense_hazard_reachability() {
    for seed in 0..30u64 {
        let mut state = seed + 1;
        let mut jobs = Vec::new();
        for _ in 0..40 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let names = ["a", "a/x", "a/y", "b", "b/x", "other"];
            let path = names[(state >> 32) as usize % names.len()];
            let access = match state % 3 {
                0 => FileAccess::read(path),
                1 => FileAccess::write(path),
                _ => FileAccess::read_write(path),
            };
            let mut item = job(vec![access]);
            item.data_mut().unknown_effects = state % 13 == 0;
            jobs.push(item);
        }
        let mut expected = vec![vec![false; jobs.len()]; jobs.len()];
        for from in 0..jobs.len() {
            for to in from + 1..jobs.len() {
                expected[from][to] = jobs[from].data().unknown_effects
                    || jobs[to].data().unknown_effects
                    || jobs[from].data().accesses.iter().any(|a| {
                        jobs[to].data().accesses.iter().any(|b| {
                            overlap(&a.path, &b.path) && (a.access.writes() || b.access.writes())
                        })
                    });
            }
        }
        for via in 0..jobs.len() {
            for from in 0..jobs.len() {
                for to in 0..jobs.len() {
                    let through = expected[from][via] && expected[via][to];
                    expected[from][to] |= through;
                }
            }
        }
        let dag = create_graph(jobs).unwrap();
        for (from, row) in expected.iter().enumerate() {
            for (to, &reachable) in row.iter().enumerate() {
                assert_eq!(
                    reaches(&dag, from, to),
                    reachable,
                    "seed {seed}: {from} -> {to}"
                );
            }
        }
    }
}

#[test]
fn optimization_removes_real_transitive_edges_and_preserves_generic_reachability() {
    let mut dag = create_graph((0..20).map(|_| job(Vec::new())).collect()).unwrap();
    dag.edges = (0..20)
        .flat_map(|from| {
            (from + 1..20)
                .filter(move |to| (from * 7 + to * 3) % 5 != 0)
                .map(move |to| Edge { from, to })
        })
        .collect();
    dag.waves = dag.topological_waves().unwrap();
    let waves = dag.waves.clone();
    let before: Vec<_> = (0..20)
        .flat_map(|from| (0..20).map(move |to| (from, to)))
        .map(|(from, to)| reaches(&dag, from, to))
        .collect();
    let original_edges = dag.edges.len();
    optimize_graph(&mut dag).unwrap();
    assert!(dag.edges.len() < original_edges);
    assert_eq!(dag.waves, waves);
    let after: Vec<_> = (0..20)
        .flat_map(|from| (0..20).map(move |to| (from, to)))
        .map(|(from, to)| reaches(&dag, from, to))
        .collect();
    assert_eq!(before, after);
    for index in 0..dag.edges.len() {
        let edge = dag.edges.remove(index);
        assert!(!reaches(&dag, edge.from, edge.to));
        dag.edges.insert(index, edge);
    }
}
