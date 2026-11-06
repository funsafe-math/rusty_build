pub mod cli;
pub mod commands;
pub mod graph;
pub mod headers;
pub mod input;
pub mod job;
pub mod registry;
pub mod rewrite;
pub mod scheduler;
pub mod shell;

pub use graph::{Dag, create_graph};
pub use rewrite::optimize_graph;
pub use scheduler::execute;

use anyhow::Result;
use job::Job;
use registry::Registry;

/// Example preprocessing policy, deliberately off for the initial core.
pub const ENABLE_HEADER_PREPROCESSING: bool = false;

pub fn parse(text: &str, registry: &Registry) -> Result<Vec<Box<dyn Job>>> {
    input::read_log(text)?
        .into_iter()
        .map(|line| registry.parse(line))
        .collect()
}

pub fn run(text: &str, registry: &Registry, verbose: bool) -> Result<()> {
    let mut jobs = parse(text, registry)?;
    if ENABLE_HEADER_PREPROCESSING {
        let existing = headers::discover_headers(&jobs)?;
        headers::add_header_dependencies(&mut jobs, existing)?;
    }
    let mut dag = create_graph(jobs)?;
    optimize_graph(&mut dag)?;
    execute(&dag, verbose)
}
