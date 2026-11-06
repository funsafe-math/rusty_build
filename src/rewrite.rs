use crate::graph::{Dag, Edge};
use anyhow::Result;

/// Remove redundant prerequisites without changing any reachable job pair.
pub fn optimize_graph(dag: &mut Dag) -> Result<()> {
    let waves = dag.topological_waves()?;
    let mut rank = vec![0; dag.jobs.len()];
    for (position, &node) in waves.iter().flatten().enumerate() {
        rank[node] = position;
    }
    let mut successors = vec![Vec::new(); dag.jobs.len()];
    for edge in &dag.edges {
        successors[edge.from].push(edge.to);
    }
    for children in &mut successors {
        children.sort_unstable_by_key(|&node| rank[node]);
        children.dedup();
    }
    let mut reduced = Vec::new();
    let mut reached = vec![usize::MAX; dag.jobs.len()];
    let mut pending = Vec::new();
    for (from, children) in successors.iter().enumerate() {
        if children.len() <= 1 {
            reduced.extend(children.iter().map(|&to| Edge { from, to }));
            continue;
        }
        for &to in children {
            if reached[to] == from {
                continue;
            }
            reduced.push(Edge { from, to });
            pending.push(to);
            while let Some(node) = pending.pop() {
                if reached[node] != from {
                    reached[node] = from;
                    pending.extend(successors[node].iter().copied());
                }
            }
        }
    }
    reduced.sort_unstable_by_key(|edge| (edge.from, edge.to));
    dag.edges = reduced;
    dag.waves = waves;
    Ok(())
}
