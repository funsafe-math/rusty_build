//! Opt-in dependencies on declared generated headers, not general header discovery.

use crate::{
    graph::{BuildGraph, RequiredDependency, normalize},
    job::JobRecord,
};
use anyhow::{Context, Result};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

#[derive(Debug, Default, Serialize)]
pub struct HeaderDependencyStats {
    pub selected_jobs: usize,
    pub added_inputs: usize,
    pub added_dependencies: usize,
}

fn header(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            [
                "h", "hh", "hpp", "hxx", "h++", "inc", "inl", "ipp", "tpp", "tcc", "cuh",
            ]
            .iter()
            .any(|suffix| extension.eq_ignore_ascii_case(suffix))
        })
}

/// Add all declared generated headers beneath a selected compiler's include dirs.
///
/// The predicate sees original source spellings from compiler metadata. All writers
/// of each matching header become prerequisites, including later known writers.
/// Neither command text nor argv is changed; undeclared effects are never guessed.
/// A failed graph rebuild restores the original inputs and required dependencies.
pub fn add_generated_header_dependencies<P>(
    graph: &mut BuildGraph,
    predicate: P,
) -> Result<HeaderDependencyStats>
where
    P: Fn(&JobRecord, &Path) -> bool,
{
    let mut writers = BTreeMap::<PathBuf, BTreeSet<usize>>::new();
    for (node, job) in graph.jobs.iter().enumerate() {
        for output in &job.spec.outputs {
            if header(output) {
                writers
                    .entry(normalize(&graph.cwd, output))
                    .or_default()
                    .insert(node);
            }
        }
    }

    let original_input_lengths: Vec<_> = graph.jobs.iter().map(|j| j.spec.inputs.len()).collect();
    let original_dependency_count = graph.required_dependencies.len();
    // Existing records can name an original job now fused into another node.
    let source_nodes: BTreeMap<_, _> = graph
        .provenance
        .iter()
        .enumerate()
        .flat_map(|(node, originals)| originals.iter().map(move |job| (job.source.line, node)))
        .collect();
    let mut required: BTreeSet<_> = graph
        .required_dependencies
        .iter()
        .filter_map(|dependency| {
            Some((
                *source_nodes.get(&dependency.producer_line)?,
                *source_nodes.get(&dependency.consumer_line)?,
                normalize(&graph.cwd, &dependency.artifact),
            ))
        })
        .collect();
    let mut stats = HeaderDependencyStats::default();
    let source_lines: Vec<_> = graph.jobs.iter().map(|job| job.source.line).collect();
    for (consumer, job) in graph.jobs.iter_mut().enumerate() {
        if !matches!(job.spec.kind.as_str(), "compile" | "link")
            || !job.spec.parsed["sources"]
                .as_array()
                .is_some_and(|sources| {
                    sources
                        .iter()
                        .filter_map(|source| source.as_str())
                        .any(|source| predicate(job, Path::new(source)))
                })
        {
            continue;
        }
        stats.selected_jobs += 1;
        let directories: BTreeSet<_> = job.spec.parsed["includes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|directory| directory.as_str())
            .map(|directory| normalize(&graph.cwd, Path::new(directory)))
            .collect();
        let mut inputs: BTreeSet<_> = job
            .spec
            .inputs
            .iter()
            .map(|input| normalize(&graph.cwd, input))
            .collect();
        let outputs: BTreeSet<_> = job
            .spec
            .outputs
            .iter()
            .map(|output| normalize(&graph.cwd, output))
            .collect();
        for (artifact, producers) in &writers {
            if outputs.contains(artifact)
                || !directories
                    .iter()
                    .any(|directory| artifact.starts_with(directory))
            {
                continue;
            }
            if inputs.insert(artifact.clone()) {
                job.spec.inputs.push(artifact.clone());
                stats.added_inputs += 1;
            }
            for &producer in producers {
                if producer == consumer {
                    continue;
                }
                if required.insert((producer, consumer, artifact.clone())) {
                    graph.required_dependencies.push(RequiredDependency {
                        producer_line: source_lines[producer],
                        consumer_line: job.source.line,
                        artifact: artifact.clone(),
                        reason: "generated header beneath selected compiler include directory"
                            .into(),
                    });
                    stats.added_dependencies += 1;
                }
            }
        }
    }

    if stats.added_inputs != 0 || stats.added_dependencies != 0 {
        if let Err(error) = graph.rebuild() {
            for (job, length) in graph.jobs.iter_mut().zip(original_input_lengths) {
                job.spec.inputs.truncate(length);
            }
            graph
                .required_dependencies
                .truncate(original_dependency_count);
            graph
                .rebuild()
                .context("restore graph after generated-header dependency failure")?;
            return Err(error).context("add generated-header dependencies");
        }
        graph.decisions.push(format!(
            "generated headers: {} selected compiler jobs, {} added inputs, {} added required dependencies",
            stats.selected_jobs, stats.added_inputs, stats.added_dependencies,
        ));
    }
    Ok(stats)
}
