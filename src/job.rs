use crate::{input::SourceCommand, shell::ShellCommand};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    any::Any,
    fmt::Debug,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExecutionStrategy {
    External,
    Batched,
    Internal,
}

/// A command's typed behavior. Implementations live alongside their argument
/// grammar; graph algorithms consume JobRecord data, never concrete job kinds.
/// Registration supplies the object-safe adapter for this intentionally typed API.
pub trait Job: Debug + Serialize + DeserializeOwned + Send + Sync + 'static {
    const TYPE: &'static str;
    const VERSION: u32 = 1;
    fn patterns() -> Vec<String> {
        Vec::new()
    }
    fn parse(shell: &ShellCommand) -> Result<Self>;
    fn kind(&self) -> &str {
        Self::TYPE
    }
    fn inputs(&self) -> impl Iterator<Item = &Path>;
    fn outputs(&self) -> impl Iterator<Item = &Path>;
    fn strategy(&self) -> ExecutionStrategy {
        ExecutionStrategy::External
    }
    fn unknown_effects(&self) -> bool {
        false
    }
    fn cacheable(&self) -> bool {
        false
    }
    fn creates_parents(&self) -> bool {
        false
    }
    fn refine_effects(&self, _spec: &mut JobSpec, _cwd: &Path) -> Result<()> {
        Ok(())
    }
    fn validate(&self, _record: &JobRecord, _cwd: &Path) -> Result<()> {
        Ok(())
    }
    fn execute(&self, context: &ExecutionContext<'_>) -> Result<()>;
    fn execute_batch(items: &[BatchItem<'_, Self>], context: &BatchContext<'_>) -> BatchReport {
        context.run_each(items, |job, execution| job.execute(execution))
    }
}

/// Persistent data, including conservative graph-policy overlays. `parsed` is
/// the serialized typed job payload, not a lossy argv reconstruction.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobSpec {
    pub type_tag: String,
    pub handler_version: u32,
    pub kind: String,
    pub inputs: Vec<PathBuf>,
    pub outputs: Vec<PathBuf>,
    pub unknown_effects: bool,
    pub cacheable: bool,
    pub creates_parents: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_reason: Option<String>,
    pub parsed: serde_json::Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct JobRecord {
    pub source: SourceCommand,
    pub shell: Option<ShellCommand>,
    pub spec: JobSpec,
    pub strategy: ExecutionStrategy,
    #[serde(skip)]
    command: Arc<dyn ErasedJob>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobSnapshot {
    pub source: SourceCommand,
    pub shell: Option<ShellCommand>,
    pub spec: JobSpec,
    pub strategy: ExecutionStrategy,
}

impl JobRecord {
    pub fn new<T: Job>(
        source: SourceCommand,
        shell: Option<ShellCommand>,
        command: T,
    ) -> Result<Self> {
        if T::TYPE.is_empty() {
            bail!("job type tag must not be empty");
        }
        let spec = JobSpec {
            type_tag: T::TYPE.into(),
            handler_version: T::VERSION,
            kind: command.kind().into(),
            inputs: command.inputs().map(Path::to_path_buf).collect(),
            outputs: command.outputs().map(Path::to_path_buf).collect(),
            unknown_effects: command.unknown_effects(),
            cacheable: command.cacheable(),
            creates_parents: command.creates_parents(),
            effect_reason: None,
            parsed: serde_json::to_value(&command)?,
        };
        let mut record = Self {
            source,
            shell,
            strategy: command.strategy(),
            spec,
            command: Arc::new(command),
        };
        record.apply_redirections();
        Ok(record)
    }
    pub fn typed<T: Job>(&self) -> Option<&T> {
        self.command.as_any().downcast_ref()
    }
    pub fn replace<T: Job>(&mut self, command: T) -> Result<()> {
        let replacement = Self::new(self.source.clone(), self.shell.clone(), command)?;
        *self = replacement;
        Ok(())
    }
    pub fn refine_effects(&mut self, cwd: &Path) -> Result<()> {
        self.command.refine_effects(&mut self.spec, cwd)
    }
    pub(crate) fn restore<T: Job>(snapshot: JobSnapshot) -> Result<Self> {
        if snapshot.spec.type_tag != T::TYPE || snapshot.spec.handler_version != T::VERSION {
            bail!(
                "job handler/version mismatch for {}",
                snapshot.spec.type_tag
            );
        }
        let command = T::deserialize(&snapshot.spec.parsed)
            .with_context(|| format!("decode typed job {}", T::TYPE))?;
        if snapshot.spec.kind != command.kind() {
            bail!("job kind is inconsistent with typed payload {}", T::TYPE);
        }
        let redirected = has_redirects(snapshot.shell.as_ref());
        let replaces_effects = redirected
            && (command.strategy() != ExecutionStrategy::External || command.unknown_effects());
        let expected_strategy = if redirected {
            ExecutionStrategy::External
        } else {
            command.strategy()
        };
        if snapshot.strategy != expected_strategy {
            bail!(
                "execution strategy is inconsistent with typed payload {}",
                T::TYPE
            );
        }
        if (command.unknown_effects() || replaces_effects) && !snapshot.spec.unknown_effects {
            bail!(
                "snapshot cannot remove intrinsic unknown effects for {}",
                T::TYPE
            );
        }
        if snapshot.spec.cacheable && (!command.cacheable() || redirected) {
            bail!("snapshot cannot enable unsafe caching for {}", T::TYPE);
        }
        if !replaces_effects {
            if command
                .inputs()
                .any(|p| !snapshot.spec.inputs.iter().any(|stored| stored == p))
                || command
                    .outputs()
                    .any(|p| !snapshot.spec.outputs.iter().any(|stored| stored == p))
            {
                bail!("snapshot omits intrinsic file effects for {}", T::TYPE);
            }
            if command.creates_parents() && !snapshot.spec.creates_parents {
                bail!(
                    "snapshot omits intrinsic parent-directory effects for {}",
                    T::TYPE
                );
            }
        }
        if let Some(shell) = &snapshot.shell {
            if shell
                .stdin
                .iter()
                .chain(shell.stdout.iter().filter(|_| shell.append_stdout))
                .any(|path| !snapshot.spec.inputs.contains(path))
                || shell
                    .stdout
                    .iter()
                    .chain(shell.stderr.iter())
                    .any(|path| !snapshot.spec.outputs.contains(path))
            {
                bail!("snapshot omits shell redirection effects for {}", T::TYPE);
            }
        }
        Ok(Self {
            source: snapshot.source,
            shell: snapshot.shell,
            spec: snapshot.spec,
            strategy: snapshot.strategy,
            command: Arc::new(command),
        })
    }
    fn apply_redirections(&mut self) {
        let Some(shell) = &self.shell else {
            return;
        };
        if !has_redirects(Some(shell)) {
            return;
        }
        let precise_external =
            self.strategy == ExecutionStrategy::External && !self.spec.unknown_effects;
        self.strategy = ExecutionStrategy::External;
        if !precise_external {
            self.spec.inputs.clear();
            self.spec.outputs.clear();
            self.spec.unknown_effects = true;
            self.spec.creates_parents = false;
        }
        if let Some(path) = &shell.stdin {
            self.spec.inputs.push(path.clone());
        }
        if let Some(path) = &shell.stdout {
            self.spec.outputs.push(path.clone());
            if shell.append_stdout {
                self.spec.inputs.push(path.clone());
            }
        }
        if let Some(path) = &shell.stderr {
            self.spec.outputs.push(path.clone());
        }
        self.spec.cacheable = false;
    }
}

pub fn has_redirects(shell: Option<&ShellCommand>) -> bool {
    shell.is_some_and(|s| s.stdin.is_some() || s.stdout.is_some() || s.stderr.is_some())
}

pub struct ExecutionContext<'a> {
    pub cwd: &'a Path,
    pub record: &'a JobRecord,
}
impl ExecutionContext<'_> {
    pub fn absolute(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_owned()
        } else {
            self.cwd.join(path)
        }
    }
}

pub struct BatchItem<'a, T: Job> {
    pub id: usize,
    pub job: &'a T,
    pub record: &'a JobRecord,
}
pub struct BatchContext<'a> {
    pub cwd: &'a Path,
    pub cancelled: &'a AtomicBool,
}
#[derive(Default, Debug)]
pub struct BatchReport {
    pub outcomes: Vec<(usize, Result<()>)>,
}
impl BatchReport {
    fn failed(id: usize, error: anyhow::Error) -> Self {
        Self {
            outcomes: vec![(id, Err(error))],
        }
    }
}
impl BatchContext<'_> {
    pub fn run_each<T: Job, F>(&self, items: &[BatchItem<'_, T>], mut execute: F) -> BatchReport
    where
        F: FnMut(&T, &ExecutionContext<'_>) -> Result<()>,
    {
        let mut report = BatchReport {
            outcomes: Vec::with_capacity(items.len()),
        };
        for item in items {
            if self.cancelled.load(Ordering::Acquire) {
                break;
            }
            let context = ExecutionContext {
                cwd: self.cwd,
                record: item.record,
            };
            let result = if item.record.strategy == ExecutionStrategy::External
                && item.job.strategy() != ExecutionStrategy::External
            {
                crate::commands::external::execute_original(&context)
            } else {
                item.job
                    .validate(item.record, self.cwd)
                    .and_then(|()| execute(item.job, &context))
            };
            if result.is_err() {
                self.cancelled.store(true, Ordering::Release);
            }
            report.outcomes.push((item.id, result));
        }
        report
    }
}

trait ErasedJob: Debug + Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn refine_effects(&self, spec: &mut JobSpec, cwd: &Path) -> Result<()>;
    fn execute_batch(
        &self,
        records: &[JobRecord],
        ids: &[usize],
        context: &BatchContext<'_>,
    ) -> BatchReport;
}
impl<T: Job> ErasedJob for T {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn refine_effects(&self, spec: &mut JobSpec, cwd: &Path) -> Result<()> {
        Job::refine_effects(self, spec, cwd)
    }
    fn execute_batch(
        &self,
        records: &[JobRecord],
        ids: &[usize],
        context: &BatchContext<'_>,
    ) -> BatchReport {
        let mut items = Vec::with_capacity(ids.len());
        for &id in ids {
            let Some(record) = records.get(id) else {
                context.cancelled.store(true, Ordering::Release);
                return BatchReport::failed(
                    id,
                    anyhow::anyhow!("batch contains invalid node {id}"),
                );
            };
            let Some(job) = record.typed::<T>() else {
                context.cancelled.store(true, Ordering::Release);
                return BatchReport::failed(
                    id,
                    anyhow::anyhow!("heterogeneous typed batch for {}", T::TYPE),
                );
            };
            items.push(BatchItem { id, job, record });
        }
        let report = T::execute_batch(&items, context);
        validate_report(ids, report, context)
    }
}

/// Called once per concrete-type/identity/strategy group, with ascending node IDs.
/// No operation match or tool-name special case exists in the scheduler.
pub fn execute_batch(
    records: &[JobRecord],
    ids: &[usize],
    context: &BatchContext<'_>,
) -> BatchReport {
    let Some(&id) = ids.first() else {
        return BatchReport::default();
    };
    if ids.windows(2).any(|pair| pair[0] >= pair[1]) {
        context.cancelled.store(true, Ordering::Release);
        return BatchReport::failed(
            id,
            anyhow::anyhow!("batch node IDs must be unique and ordered"),
        );
    }
    let Some(first) = records.get(id) else {
        context.cancelled.store(true, Ordering::Release);
        return BatchReport::failed(id, anyhow::anyhow!("batch contains invalid node {id}"));
    };
    first.command.execute_batch(records, ids, context)
}

fn validate_report(ids: &[usize], report: BatchReport, context: &BatchContext<'_>) -> BatchReport {
    let mut seen = vec![false; ids.len()];
    let mut failed = false;
    for (id, result) in &report.outcomes {
        let index = match ids.binary_search(id) {
            Ok(index) if !seen[index] => index,
            _ => {
                context.cancelled.store(true, Ordering::Release);
                return BatchReport::failed(
                    ids[0],
                    anyhow::anyhow!("batch handler reported an unknown or duplicate node {id}"),
                );
            }
        };
        seen[index] = true;
        failed |= result.is_err();
    }
    if failed {
        context.cancelled.store(true, Ordering::Release);
    }
    if !failed && !context.cancelled.load(Ordering::Acquire) {
        if let Some(missing) = seen.iter().position(|&present| !present) {
            context.cancelled.store(true, Ordering::Release);
            return BatchReport::failed(
                ids[missing],
                anyhow::anyhow!("batch handler omitted a result for node {}", ids[missing]),
            );
        }
    }
    report
}
