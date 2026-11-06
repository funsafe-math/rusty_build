use crate::{graph::Dag, job::Job};
use anyhow::{Context, Result};

/// Execute dependency-safe, concrete-type batches synchronously. Validate every
/// edge before checking inputs or allowing the first command to have effects.
pub fn execute(dag: &Dag, verbose: bool) -> Result<()> {
    let waves = dag.topological_waves()?;
    dag.check_inputs()?;
    for batch in dag.batches_in(&waves) {
        let first = &dag.jobs[batch[0]];
        if verbose {
            eprintln!("{}: {} job(s)", first.name(), batch.len());
        }
        let items: Vec<&dyn Job> = batch
            .iter()
            .map(|&index| dag.jobs[index].as_ref())
            .collect();
        first.execute_batch(&items).with_context(|| {
            format!(
                "executing {} batch beginning at job {}",
                first.name(),
                batch[0]
            )
        })?;
    }
    Ok(())
}
