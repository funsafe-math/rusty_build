use anyhow::{Result, bail};
use rusty_make::{
    cache::Cache,
    graph::BuildGraph,
    input::SourceCommand,
    job::{
        self, BatchContext, BatchItem, BatchReport, ExecutionContext, ExecutionStrategy, Job,
        JobRecord,
    },
    parse_plan,
    registry::Registry,
    scheduler::{RunOptions, run},
    shell::ShellCommand,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Serialize, Deserialize)]
struct FileCopy {
    input: PathBuf,
    output: PathBuf,
}

fn file_arguments(shell: &ShellCommand) -> Result<(PathBuf, PathBuf)> {
    if shell.args.len() != 2 {
        bail!("expected an input and output path");
    }
    Ok((shell.args[0].clone().into(), shell.args[1].clone().into()))
}

// Audits are isolated to the test's build root, never process-global counters.
fn audit_batch<T: Job>(items: &[BatchItem<'_, T>], context: &BatchContext<'_>) {
    let mut audit = OpenOptions::new()
        .create(true)
        .append(true)
        .open(context.cwd.join(format!("{}.batches", T::TYPE)))
        .unwrap();
    let ids: Vec<_> = items.iter().map(|item| item.id).collect();
    writeln!(audit, "{}", serde_json::to_string(&ids).unwrap()).unwrap();
}

fn batches(root: &Path, type_tag: &str) -> Vec<Vec<usize>> {
    fs::read_to_string(root.join(format!("{type_tag}.batches")))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

impl Job for FileCopy {
    const TYPE: &'static str = "test.file-copy";

    fn patterns() -> Vec<String> {
        vec![r"^trait-copy(?:\s|$)".into()]
    }
    fn parse(shell: &ShellCommand) -> Result<Self> {
        let (input, output) = file_arguments(shell)?;
        Ok(Self { input, output })
    }
    fn kind(&self) -> &str {
        "file-transform"
    }
    fn inputs(&self) -> impl Iterator<Item = &Path> {
        std::iter::once(self.input.as_path())
    }
    fn outputs(&self) -> impl Iterator<Item = &Path> {
        std::iter::once(self.output.as_path())
    }
    fn strategy(&self) -> ExecutionStrategy {
        ExecutionStrategy::Batched
    }
    fn cacheable(&self) -> bool {
        true
    }
    fn execute(&self, context: &ExecutionContext<'_>) -> Result<()> {
        let bytes = fs::read(context.absolute(&self.input))?;
        if bytes == b"!fail" {
            fs::write(context.absolute(&self.output), b"partial write")?;
            bail!("input requested a failure after writing");
        }
        fs::write(context.absolute(&self.output), bytes)?;
        Ok(())
    }
    fn execute_batch(items: &[BatchItem<'_, Self>], context: &BatchContext<'_>) -> BatchReport {
        audit_batch(items, context);
        context.run_each(items, |job, execution| job.execute(execution))
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct FileReverse {
    input: PathBuf,
    output: PathBuf,
}

impl Job for FileReverse {
    const TYPE: &'static str = "test.file-reverse";

    fn patterns() -> Vec<String> {
        vec![r"^trait-reverse(?:\s|$)".into()]
    }
    fn parse(shell: &ShellCommand) -> Result<Self> {
        let (input, output) = file_arguments(shell)?;
        Ok(Self { input, output })
    }
    fn kind(&self) -> &str {
        "file-transform"
    }
    fn inputs(&self) -> impl Iterator<Item = &Path> {
        std::iter::once(self.input.as_path())
    }
    fn outputs(&self) -> impl Iterator<Item = &Path> {
        std::iter::once(self.output.as_path())
    }
    fn strategy(&self) -> ExecutionStrategy {
        ExecutionStrategy::Batched
    }
    fn cacheable(&self) -> bool {
        true
    }
    fn execute(&self, context: &ExecutionContext<'_>) -> Result<()> {
        let mut bytes = fs::read(context.absolute(&self.input))?;
        bytes.reverse();
        fs::write(context.absolute(&self.output), bytes)?;
        Ok(())
    }
    fn execute_batch(items: &[BatchItem<'_, Self>], context: &BatchContext<'_>) -> BatchReport {
        audit_batch(items, context);
        context.run_each(items, |job, execution| job.execute(execution))
    }
}

fn registry() -> Registry {
    let mut registry = Registry::empty();
    registry.register::<FileCopy>().unwrap();
    registry.register::<FileReverse>().unwrap();
    registry
}

#[test]
fn registered_job_batches_independent_work_and_executes_restored_payloads() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("a"), b"first payload\n").unwrap();
    fs::write(p.join("b"), b"second payload\n").unwrap();
    let registry = registry();
    let graph = parse_plan(
        "trait-copy a left\ntrait-copy b right\ntrait-copy left consumed\n",
        p,
        &registry,
    )
    .unwrap();
    let encoded = serde_json::to_vec(&graph).unwrap();
    drop(graph);
    let restored = BuildGraph::from_json(&encoded, &registry).unwrap();
    let report = run(
        &restored,
        &RunOptions {
            jobs: 4,
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(report.executed, 3);
    assert_eq!(batches(p, FileCopy::TYPE), vec![vec![0, 1], vec![2]]);
    assert_eq!(fs::read(p.join("left")).unwrap(), b"first payload\n");
    assert_eq!(fs::read(p.join("right")).unwrap(), b"second payload\n");
    assert_eq!(fs::read(p.join("consumed")).unwrap(), b"first payload\n");
    let cached = run(&restored, &RunOptions::default()).unwrap();
    assert_eq!((cached.executed, cached.skipped), (0, 3));
    assert_eq!(batches(p, FileCopy::TYPE), vec![vec![0, 1], vec![2]]);
}

#[test]
fn incomplete_or_incompatible_registry_rejects_plan_before_valid_prefix_executes() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("source"), b"abcde").unwrap();
    let registry = registry();
    let graph = parse_plan(
        "trait-copy source prefix\ntrait-reverse prefix consumed\n",
        p,
        &registry,
    )
    .unwrap();
    let bytes = serde_json::to_vec(&graph).unwrap();
    let mut missing_decoder = Registry::empty();
    missing_decoder.register::<FileCopy>().unwrap();
    let missing = BuildGraph::from_json(&bytes, &missing_decoder)
        .and_then(|graph| run(&graph, &RunOptions::default()));
    assert!(missing.is_err());

    let mut changed_version = serde_json::to_value(&graph).unwrap();
    // Leave the first executable job valid: rejection must be for the whole plan.
    changed_version["jobs"][1]["spec"]["handler_version"] =
        serde_json::json!(FileReverse::VERSION + 1);
    let incompatible =
        BuildGraph::from_json(&serde_json::to_vec(&changed_version).unwrap(), &registry)
            .and_then(|graph| run(&graph, &RunOptions::default()));
    assert!(incompatible.is_err());
    for name in [
        "prefix",
        "consumed",
        "test.file-copy.batches",
        "test.file-reverse.batches",
        ".rusty_make",
    ] {
        assert!(!p.join(name).exists(), "rejected plan affected {name}");
    }

    let restored = BuildGraph::from_json(&bytes, &registry).unwrap();
    run(&restored, &RunOptions::default()).unwrap();
    assert_eq!(fs::read(p.join("prefix")).unwrap(), b"abcde");
    assert_eq!(fs::read(p.join("consumed")).unwrap(), b"edcba");
}

#[test]
fn different_concrete_jobs_with_the_same_kind_have_separate_typed_batches() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("a"), b"abcd").unwrap();
    fs::write(p.join("b"), b"wxyz").unwrap();
    let registry = registry();
    let graph = parse_plan(
        "trait-copy a copied-a\ntrait-reverse a reversed-a\n\
         trait-copy b copied-b\ntrait-reverse b reversed-b\n\
         trait-copy reversed-a consumed\n",
        p,
        &registry,
    )
    .unwrap();
    let restored = BuildGraph::from_json(&serde_json::to_vec(&graph).unwrap(), &registry).unwrap();
    let report = run(
        &restored,
        &RunOptions {
            jobs: 4,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!((report.executed, report.batches), (5, 3));
    assert_eq!(batches(p, FileCopy::TYPE), vec![vec![0, 2], vec![4]]);
    assert_eq!(batches(p, FileReverse::TYPE), vec![vec![1, 3]]);
    for (name, expected) in [
        ("copied-a", b"abcd"),
        ("reversed-a", b"dcba"),
        ("copied-b", b"wxyz"),
        ("reversed-b", b"zyxw"),
        ("consumed", b"dcba"),
    ] {
        assert_eq!(fs::read(p.join(name)).unwrap(), expected);
    }
}

#[test]
fn partial_batch_failure_invalidates_old_entries_and_caches_only_completed_jobs() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    for (name, bytes) in [("a", "success"), ("b", "!fail"), ("c", "unstarted")] {
        fs::write(p.join(name), bytes).unwrap();
    }
    let graph = parse_plan(
        "trait-copy a good\ntrait-copy b failed\ntrait-copy c unstarted\n\
         trait-copy good dependent\n",
        p,
        &registry(),
    )
    .unwrap();
    // Seed valid old records, including one whose unchanged files could otherwise
    // hide that the batch never reached it. Rebuild must revoke them before work.
    for name in ["good", "failed", "unstarted"] {
        fs::write(p.join(name), b"old output").unwrap();
    }
    let mut old_cache = Cache::open(p).unwrap();
    for id in 0..3 {
        old_cache.record(&graph, id).unwrap();
        assert!(old_cache.check(&graph, id).unwrap().is_ok());
    }
    old_cache.save().unwrap();
    drop(old_cache);

    assert!(
        run(
            &graph,
            &RunOptions {
                jobs: 4,
                rebuild_all: true,
                ..Default::default()
            },
        )
        .is_err()
    );
    assert_eq!(batches(p, FileCopy::TYPE), vec![vec![0, 1, 2]]);
    assert_eq!(fs::read(p.join("good")).unwrap(), b"success");
    assert_eq!(fs::read(p.join("failed")).unwrap(), b"partial write");
    assert_eq!(fs::read(p.join("unstarted")).unwrap(), b"old output");
    assert!(!p.join("dependent").exists());
    // Even restoring the failed job's old bytes cannot resurrect its old entry.
    fs::write(p.join("failed"), b"old output").unwrap();
    let cache = Cache::open(p).unwrap();
    assert!(cache.check(&graph, 0).unwrap().is_ok());
    for id in 1..4 {
        assert!(
            cache.check(&graph, id).unwrap().is_err(),
            "cached node {id}"
        );
    }
    drop(cache);

    fs::write(p.join("b"), b"repaired").unwrap();
    let retry = run(&graph, &RunOptions::default()).unwrap();
    assert_eq!((retry.executed, retry.skipped), (3, 1));
    assert_eq!(fs::read(p.join("failed")).unwrap(), b"repaired");
    assert_eq!(fs::read(p.join("unstarted")).unwrap(), b"unstarted");
    assert_eq!(fs::read(p.join("dependent")).unwrap(), b"success");
    assert_eq!(
        batches(p, FileCopy::TYPE),
        vec![vec![0, 1, 2], vec![1, 2], vec![3]]
    );
    let cache = Cache::open(p).unwrap();
    for id in 0..4 {
        assert!(cache.check(&graph, id).unwrap().is_ok());
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
enum Malformation {
    Duplicate,
    Unknown,
    Omitted,
    Cancelled,
}

#[derive(Debug, Serialize, Deserialize)]
struct BadReporter {
    files: FileCopy,
    malformation: Malformation,
}

impl Job for BadReporter {
    const TYPE: &'static str = "test.bad-reporter";

    fn parse(_shell: &ShellCommand) -> Result<Self> {
        bail!("constructed as typed records in this test")
    }
    fn inputs(&self) -> impl Iterator<Item = &Path> {
        std::iter::once(self.files.input.as_path())
    }
    fn outputs(&self) -> impl Iterator<Item = &Path> {
        std::iter::once(self.files.output.as_path())
    }
    fn strategy(&self) -> ExecutionStrategy {
        ExecutionStrategy::Batched
    }
    fn cacheable(&self) -> bool {
        true
    }
    fn execute(&self, context: &ExecutionContext<'_>) -> Result<()> {
        let bytes = fs::read(context.absolute(&self.files.input))?;
        fs::write(context.absolute(&self.files.output), bytes)?;
        Ok(())
    }
    fn execute_batch(items: &[BatchItem<'_, Self>], context: &BatchContext<'_>) -> BatchReport {
        let mut report = context.run_each(items, |job, execution| job.execute(execution));
        match items[0].job.malformation {
            Malformation::Duplicate => report.outcomes.push((items[0].id, Ok(()))),
            Malformation::Unknown => report.outcomes.push((items.last().unwrap().id + 1, Ok(()))),
            Malformation::Omitted => {
                report.outcomes.pop();
            }
            Malformation::Cancelled => {
                context.cancelled.store(true, Ordering::Release);
                report.outcomes.clear();
            }
        }
        report
    }
}

fn malformed_graph(root: &Path, malformation: Malformation) -> BuildGraph {
    let records = (0..2)
        .map(|id| {
            JobRecord::new(
                SourceCommand {
                    line: id + 1,
                    text: format!("bad-reporter input output-{id}"),
                    target: None,
                },
                None,
                BadReporter {
                    files: FileCopy {
                        input: "input".into(),
                        output: format!("output-{id}").into(),
                    },
                    malformation,
                },
            )
            .unwrap()
        })
        .collect();
    BuildGraph::new(records, root).unwrap()
}

#[test]
fn erased_adapter_rejects_duplicate_unknown_and_omitted_result_ids() {
    for malformation in [
        Malformation::Duplicate,
        Malformation::Unknown,
        Malformation::Omitted,
    ] {
        let root = tempfile::tempdir().unwrap();
        let p = root.path();
        fs::write(p.join("input"), b"actual file work").unwrap();
        let graph = malformed_graph(p, malformation);
        let cancelled = AtomicBool::new(false);
        let context = BatchContext {
            cwd: p,
            cancelled: &cancelled,
        };
        let report = job::execute_batch(&graph.jobs, &[0, 1], &context);
        assert!(cancelled.load(Ordering::Acquire), "{malformation:?}");
        assert!(
            report.outcomes.iter().any(|(_, result)| result.is_err()),
            "{malformation:?}"
        );
        assert!(report.outcomes.iter().all(|(id, _)| *id < 2));
        for name in ["output-0", "output-1"] {
            assert_eq!(fs::read(p.join(name)).unwrap(), b"actual file work");
        }

        // The scheduler must propagate that adapter failure and not cache the
        // apparently successful filesystem work from an untrustworthy report.
        assert!(
            run(&graph, &RunOptions::default()).is_err(),
            "{malformation:?}"
        );
        let cache = Cache::open(p).unwrap();
        for id in 0..2 {
            assert!(
                cache.check(&graph, id).unwrap().is_err(),
                "{malformation:?}, node {id}"
            );
        }
    }
}

#[test]
fn batch_cancellation_without_an_error_outcome_cannot_start_dependent_work() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("input"), b"cancelled batch output").unwrap();
    let mut records = malformed_graph(p, Malformation::Cancelled).jobs;
    records.push(
        JobRecord::new(
            SourceCommand {
                line: 3,
                text: "trait-copy output-0 dependent".into(),
                target: None,
            },
            None,
            FileCopy {
                input: "output-0".into(),
                output: "dependent".into(),
            },
        )
        .unwrap(),
    );
    let graph = BuildGraph::new(records, p).unwrap();
    assert!(run(&graph, &RunOptions::default()).is_err());
    assert!(!p.join("dependent").exists());
    for id in 0..3 {
        assert!(Cache::open(p).unwrap().check(&graph, id).unwrap().is_err());
    }
}
