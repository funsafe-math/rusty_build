use crate::{
    graph::{BuildGraph, normalize},
    job::{ExecutionStrategy, Operation},
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
/// Preserve both observable output files, but read the source once when native
/// CopyChain's runtime safety checks permit it. Never elide a required artifact.
pub struct CopyChain;
impl GraphRewrite for CopyChain {
    fn name(&self) -> &'static str {
        "copy-chain shared read (intermediate preserved)"
    }
    fn apply(&self, graph: &mut BuildGraph) -> Result<RewriteStats> {
        let mut stats = RewriteStats::default();
        let mut i = 0;
        while i + 1 < graph.jobs.len() {
            let pair = match (
                &graph.jobs[i].spec.operation,
                &graph.jobs[i + 1].spec.operation,
            ) {
                (Operation::Copy { src: a, dst: b }, Operation::Copy { src: c, dst: d })
                    if normalize(&graph.cwd, b) == normalize(&graph.cwd, c)
                        && normalize(&graph.cwd, a) != normalize(&graph.cwd, d)
                        && normalize(&graph.cwd, b) != normalize(&graph.cwd, d)
                        && !graph.jobs[i].spec.unknown_effects
                        && !graph.jobs[i + 1].spec.unknown_effects =>
                {
                    Some((a.clone(), b.clone(), d.clone()))
                }
                _ => None,
            };
            if let Some((src, mid, dst)) = pair {
                let original_second = graph.jobs.remove(i + 1);
                let originals = graph.provenance.remove(i + 1);
                graph.provenance[i].extend(originals);
                let job = &mut graph.jobs[i];
                job.spec.operation = Operation::CopyChain {
                    src: src.clone(),
                    mid: mid.clone(),
                    dst: dst.clone(),
                };
                job.spec.kind = "copy-chain".into();
                job.spec.inputs = vec![src];
                job.spec.outputs = vec![mid, dst];
                job.spec.parsed = serde_json::json!({"fused_commands":[job.source.text,original_second.source.text],"intermediate_preserved":true});
                job.strategy = ExecutionStrategy::Internal;
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
        graph.rebuild()?;
        Ok(stats)
    }
}
