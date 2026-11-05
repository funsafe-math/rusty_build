use crate::{
    commands::cp::{CopyChain as CopyChainJob, Cp},
    graph::{BuildGraph, normalize},
    job::ExecutionStrategy,
};
use anyhow::Result;
use serde::Serialize;

#[derive(Debug, Default, Serialize)]
pub struct RewriteStats {
    pub transformations: usize,
    pub removed_nodes: usize,
}
pub trait GraphRewrite {
    fn name(&self) -> &'static str;
    fn apply(&self, graph: &mut BuildGraph) -> Result<RewriteStats>;
}
/// Preserve both observable outputs, sharing the source read only when the
/// command implementation's runtime safety checks permit it.
pub struct CopyChain;
impl GraphRewrite for CopyChain {
    fn name(&self) -> &'static str {
        "copy-chain shared read (intermediate preserved)"
    }
    fn apply(&self, graph: &mut BuildGraph) -> Result<RewriteStats> {
        let mut stats = RewriteStats::default();
        let mut i = 0;
        while i + 1 < graph.jobs.len() {
            let pair = match (graph.jobs[i].typed::<Cp>(), graph.jobs[i + 1].typed::<Cp>()) {
                (Some(first), Some(second))
                    if graph.jobs[i].strategy == ExecutionStrategy::Internal
                        && graph.jobs[i + 1].strategy == ExecutionStrategy::Internal
                        && !graph.jobs[i].spec.unknown_effects
                        && !graph.jobs[i + 1].spec.unknown_effects
                        && normalize(&graph.cwd, &first.dst)
                            == normalize(&graph.cwd, &second.src)
                        && normalize(&graph.cwd, &first.src)
                            != normalize(&graph.cwd, &second.dst)
                        && normalize(&graph.cwd, &first.dst)
                            != normalize(&graph.cwd, &second.dst) =>
                {
                    Some((first.src.clone(), first.dst.clone(), second.dst.clone()))
                }
                _ => None,
            };
            if let Some((src, mid, dst)) = pair {
                let second = graph.jobs.remove(i + 1);
                let originals = graph.provenance.remove(i + 1);
                graph.provenance[i].extend(originals);
                let mut inputs = std::mem::take(&mut graph.jobs[i].spec.inputs);
                inputs.extend(second.spec.inputs);
                graph.jobs[i].replace(CopyChainJob::new(src, mid.clone(), dst))?;
                for input in inputs {
                    if normalize(&graph.cwd, &input) != normalize(&graph.cwd, &mid)
                        && !graph.jobs[i]
                            .spec
                            .inputs
                            .iter()
                            .any(|p| normalize(&graph.cwd, p) == normalize(&graph.cwd, &input))
                    {
                        graph.jobs[i].spec.inputs.push(input);
                    }
                }
                stats.transformations += 1;
                stats.removed_nodes += 1;
            }
            i += 1;
        }
        graph.decisions.push(format!(
            "{}: {} transformations; all intermediate files retained",
            self.name(),
            stats.transformations
        ));
        if stats.transformations != 0 {
            graph.rebuild()?;
        }
        Ok(stats)
    }
}
