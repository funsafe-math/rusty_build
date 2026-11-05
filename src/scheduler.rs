use crate::{cache::Cache, graph::BuildGraph, job};
use anyhow::{Context, Result, bail};
use rayon::prelude::*;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Debug)]
pub struct RunOptions {
    pub jobs: usize,
    pub rebuild_all: bool,
    pub explain: bool,
    pub dry_run: bool,
}
impl Default for RunOptions {
    fn default() -> Self {
        Self {
            jobs: 1,
            rebuild_all: false,
            explain: false,
            dry_run: false,
        }
    }
}
#[derive(Default, Debug, Serialize)]
pub struct RunReport {
    pub executed: usize,
    pub skipped: usize,
    pub batches: usize,
    pub planned: usize,
}
struct BuildLock(PathBuf);
impl Drop for BuildLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn lock(graph: &BuildGraph) -> Result<BuildLock> {
    let dir = graph.cwd.join(".rusty_make");
    fs::create_dir_all(&dir)?;
    let path = dir.join("build.lock");
    fs::OpenOptions::new().create_new(true).write(true).open(&path).context("build root locked: another rusty_make is running, or remove stale .rusty_make/build.lock after checking no build runs")?;
    Ok(BuildLock(path))
}

/// Each wave is dependency-safe; grouping is separate from topological sorting.
/// Native groups avoid process startup; External groups intentionally execute
/// original commands separately (no invented compiler batching semantics).
pub fn run(graph: &BuildGraph, options: &RunOptions) -> Result<RunReport> {
    if options.jobs == 0 {
        bail!("--jobs must be at least 1");
    }
    graph.topological_waves()?; // validate caller-supplied graphs before effects
    let _lock = if options.dry_run {
        None
    } else {
        Some(lock(graph)?)
    };
    let mut cache = Cache::open(&graph.cwd)?;
    let mut report = RunReport::default();
    let mut unvalidated = vec![false; graph.jobs.len()];
    let mut pool = None;
    for (wave_index, wave) in graph.topological_waves()?.iter().enumerate() {
        let mut groups = BTreeMap::<(&str, &str, job::ExecutionStrategy), Vec<usize>>::new();
        for &id in wave {
            unvalidated[id] = graph
                .edges
                .iter()
                .filter(|e| e.to == id)
                .any(|e| unvalidated[e.from] || graph.jobs[e.from].spec.unknown_effects);
            let decision = if options.rebuild_all {
                Err("explicit --rebuild-all".into())
            } else if unvalidated[id] {
                Err("an unknown-effect dependency cannot be validated".into())
            } else {
                cache.check(graph, id)?
            };
            match decision {
                Ok(()) => {
                    report.skipped += 1;
                    if options.explain {
                        eprintln!("[CACHE] hit node {id}: {}", graph.jobs[id].source.text);
                    }
                }
                Err(reason) => {
                    if options.explain {
                        eprintln!("[CACHE] miss node {id}: {reason}");
                    }
                    groups
                        .entry((
                            graph.jobs[id].spec.type_tag.as_str(),
                            graph.jobs[id].spec.kind.as_str(),
                            graph.jobs[id].strategy,
                        ))
                        .or_default()
                        .push(id);
                }
            }
        }
        let groups: Vec<_> = groups.into_iter().collect();
        report.batches += groups.len();
        let planned = groups.iter().map(|(_, ids)| ids.len()).sum::<usize>();
        report.planned += planned;
        for ((type_tag, kind, strategy), ids) in &groups {
            if options.explain || options.dry_run {
                eprintln!(
                    "[BATCH] wave {wave_index} {kind}:{strategy:?} ({type_tag}): nodes {ids:?}"
                );
            }
        }
        if options.dry_run || groups.is_empty() {
            continue;
        }
        if pool.is_none() {
            pool = Some(
                rayon::ThreadPoolBuilder::new()
                    .num_threads(options.jobs.min(graph.jobs.len()))
                    .build()
                    .context("creating scheduler worker pool")?,
            );
        }
        for (_, ids) in &groups {
            for &id in ids {
                cache.invalidate(graph, id);
            }
        }
        cache.save()?;
        let failed = AtomicBool::new(false);
        let results = Mutex::new(Vec::with_capacity(planned));
        pool.as_ref().unwrap().install(|| {
            groups.par_iter().for_each(|(_, ids)| {
                if failed.load(Ordering::Acquire) {
                    return;
                }
                let context = job::BatchContext {
                    cwd: &graph.cwd,
                    cancelled: &failed,
                };
                let report = job::execute_batch(&graph.jobs, ids, &context);
                results
                    .lock()
                    .unwrap()
                    .extend(report.outcomes.into_iter().map(|(id, result)| {
                        (
                            id,
                            result.with_context(|| {
                                format!(
                                    "node {id}, input line {}: {}",
                                    graph.jobs[id].source.line, graph.jobs[id].source.text
                                )
                            }),
                        )
                    }));
            });
        });
        let mut results = results.into_inner().unwrap();
        results.sort_by_key(|(id, _)| *id);
        let mut first_error = None;
        for (id, result) in results {
            match result {
                Ok(()) => {
                    report.executed += 1;
                    cache.record(graph, id)?;
                    if options.explain {
                        eprintln!("[EXEC] completed node {id}");
                    }
                }
                Err(error) => {
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
        }
        cache.save()?;
        if let Some(error) = first_error {
            return Err(error);
        }
        if failed.load(Ordering::Acquire) {
            bail!("batch execution cancelled without an error outcome");
        }
    }
    Ok(report)
}
