pub mod cache;
pub mod cli;
pub mod commands;
pub mod compdb;
pub mod graph;
pub mod headers;
pub mod input;
pub mod job;
pub mod make;
pub mod registry;
pub mod rewrite;
pub mod scheduler;
pub mod shell;

use anyhow::Result;
use std::path::Path;

/// Validate every command before exposing an executable graph.
pub fn parse_plan(
    text: &str,
    cwd: &Path,
    registry: &registry::Registry,
) -> Result<graph::BuildGraph> {
    let log = input::read_log(text)?;
    plan_log(log, cwd, registry)
}
pub fn plan_log(
    log: input::InputLog,
    cwd: &Path,
    registry: &registry::Registry,
) -> Result<graph::BuildGraph> {
    let (commands, diagnostics) = log.into_parts();
    let jobs = commands
        .into_iter()
        .map(|record| registry.classify_parsed(record))
        .collect::<Result<Vec<_>>>()?;
    let mut graph = graph::BuildGraph::new(jobs, cwd)?;
    graph.diagnostics = diagnostics;
    Ok(graph)
}
