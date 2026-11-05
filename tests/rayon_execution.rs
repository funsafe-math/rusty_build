use anyhow::{Result, bail};
use rusty_make::{
    graph::BuildGraph,
    input::SourceCommand,
    job::{ExecutionContext, ExecutionStrategy, Job, JobRecord},
    scheduler::{RunOptions, run},
    shell::ShellCommand,
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

#[derive(Default)]
struct Activity {
    active: usize,
    peak: usize,
    arrivals: usize,
}
#[derive(Default)]
struct Gate {
    activity: Mutex<Activity>,
    changed: Condvar,
}
impl std::fmt::Debug for Gate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Gate")
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct ParallelFile {
    output: PathBuf,
    payload: String,
    #[serde(skip)]
    gate: Arc<Gate>,
    #[serde(skip)]
    required_arrivals: usize,
}
impl Job for ParallelFile {
    const TYPE: &'static str = "test.parallel-file";
    fn parse(_: &ShellCommand) -> Result<Self> {
        bail!("parallel-file probes are constructed directly")
    }
    fn kind(&self) -> &str {
        &self.payload
    }
    fn inputs(&self) -> impl Iterator<Item = &Path> {
        std::iter::empty()
    }
    fn outputs(&self) -> impl Iterator<Item = &Path> {
        std::iter::once(self.output.as_path())
    }
    fn strategy(&self) -> ExecutionStrategy {
        ExecutionStrategy::Internal
    }
    fn execute(&self, context: &ExecutionContext<'_>) -> Result<()> {
        let mut activity = self.gate.activity.lock().unwrap();
        activity.active += 1;
        activity.peak = activity.peak.max(activity.active);
        activity.arrivals += 1;
        self.gate.changed.notify_all();
        let (activity, timeout) = self
            .gate
            .changed
            .wait_timeout_while(activity, Duration::from_secs(10), |activity| {
                activity.arrivals < self.required_arrivals
            })
            .unwrap();
        if timeout.timed_out() {
            bail!("independent batches did not make parallel progress");
        }
        drop(activity);
        let written = fs::write(context.absolute(&self.output), &self.payload);
        self.gate.activity.lock().unwrap().active -= 1;
        written?;
        Ok(())
    }
}

#[test]
fn independent_batches_make_parallel_progress_without_exceeding_jobs() {
    for workers in [1, 2] {
        let root = tempfile::tempdir().unwrap();
        let gate = Arc::new(Gate::default());
        let records = (0..6)
            .map(|index| {
                JobRecord::new(
                    SourceCommand {
                        line: index + 1,
                        text: format!("parallel-file output-{index}"),
                        target: None,
                    },
                    None,
                    ParallelFile {
                        output: format!("output-{index}").into(),
                        payload: format!("payload-{index}"),
                        gate: gate.clone(),
                        required_arrivals: workers,
                    },
                )
                .unwrap()
            })
            .collect();
        let graph = BuildGraph::new(records, root.path()).unwrap();
        let report = run(
            &graph,
            &RunOptions {
                jobs: workers,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!((report.executed, report.batches), (6, 6));
        assert_eq!(gate.activity.lock().unwrap().peak, workers);
        for index in 0..6 {
            assert_eq!(
                fs::read_to_string(root.path().join(format!("output-{index}"))).unwrap(),
                format!("payload-{index}")
            );
        }
    }
}
