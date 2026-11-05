use crate::job::{JobRecord, JobSnapshot};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArtifactId {
    pub path: PathBuf,
    pub version: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub reason: String,
}

/// An explicit semantic prerequisite, identified by original input lines so it
/// remains valid when rewrites fuse executable nodes and retain provenance.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequiredDependency {
    pub producer_line: usize,
    pub consumer_line: usize,
    pub artifact: PathBuf,
    pub reason: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct BuildGraph {
    pub jobs: Vec<JobRecord>,
    pub edges: Vec<Edge>,
    pub required_dependencies: Vec<RequiredDependency>,
    pub waves: Vec<Vec<usize>>,
    pub cwd: PathBuf,
    pub reads: Vec<Vec<ArtifactId>>,
    pub writes: Vec<Vec<ArtifactId>>,
    pub decisions: Vec<String>,
    pub diagnostics: Vec<crate::input::SourceDiagnostic>,
    /// Original jobs replaced by each executable node, in their original order.
    pub provenance: Vec<Vec<JobRecord>>,
}

#[derive(Deserialize)]
struct StoredGraph {
    jobs: Vec<JobSnapshot>,
    provenance: Vec<Vec<JobSnapshot>>,
    cwd: PathBuf,
    #[serde(default)]
    edges: Vec<Edge>,
    #[serde(default)]
    required_dependencies: Vec<RequiredDependency>,
    #[serde(default)]
    decisions: Vec<String>,
    #[serde(default)]
    diagnostics: Vec<crate::input::SourceDiagnostic>,
}

/// Resolve existing ancestors to expose aliases, then normalize missing suffixes.
/// Backslash is not a path separator on POSIX.
pub fn normalize(cwd: &Path, path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        cwd.join(path)
    };
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            c => {
                resolved.push(c.as_os_str());
                if let Ok(existing) = resolved.canonicalize() {
                    resolved = existing;
                }
            }
        }
    }
    resolved
}
fn overlaps(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

impl BuildGraph {
    pub fn new(jobs: Vec<JobRecord>, cwd: &Path) -> Result<Self> {
        let cwd = cwd
            .canonicalize()
            .context("resolve build working directory")?;
        let mut graph = Self {
            provenance: jobs.iter().cloned().map(|j| vec![j]).collect(),
            jobs,
            edges: vec![],
            required_dependencies: vec![],
            waves: vec![],
            cwd,
            reads: vec![],
            writes: vec![],
            decisions: vec![],
            diagnostics: vec![],
        };
        graph.rebuild()?;
        Ok(graph)
    }
    /// Rehydrate every typed job using an explicit registry before exposing a
    /// runnable graph. Unknown types/versions fail; no placeholder executors.
    pub fn from_json(bytes: &[u8], registry: &crate::registry::Registry) -> Result<Self> {
        let stored: StoredGraph = serde_json::from_slice(bytes).context("decode execution plan")?;
        let jobs = stored
            .jobs
            .into_iter()
            .map(|job| registry.restore(job))
            .collect::<Result<Vec<_>>>()?;
        let provenance = stored
            .provenance
            .into_iter()
            .map(|originals| {
                originals
                    .into_iter()
                    .map(|job| registry.restore(job))
                    .collect::<Result<Vec<_>>>()
            })
            .collect::<Result<Vec<_>>>()?;
        if provenance.len() != jobs.len() {
            bail!("plan provenance does not match executable node count");
        }
        if jobs.iter().zip(&provenance).any(|(job, originals)| {
            !originals
                .iter()
                .any(|original| original.source.line == job.source.line)
        }) {
            bail!("plan provenance omits an executable node's source identity");
        }
        let mut graph = Self {
            jobs,
            provenance,
            cwd: stored
                .cwd
                .canonicalize()
                .context("resolve stored build directory")?,
            edges: vec![],
            required_dependencies: stored.required_dependencies,
            waves: vec![],
            reads: vec![],
            writes: vec![],
            decisions: stored.decisions,
            diagnostics: stored.diagnostics,
        };
        graph.rebuild()?;
        for edge in stored.edges {
            if !graph
                .edges
                .iter()
                .any(|existing| existing.from == edge.from && existing.to == edge.to)
            {
                graph.edges.push(edge);
            }
        }
        graph.waves = graph.topological_waves()?;
        Ok(graph)
    }

    pub fn rebuild(&mut self) -> Result<()> {
        self.edges.clear();
        self.reads.clear();
        self.writes.clear();
        for job in &mut self.jobs {
            job.refine_effects(&self.cwd)?;
        }
        let mut versions = std::collections::BTreeMap::<PathBuf, u32>::new();
        let inputs: Vec<Vec<PathBuf>> = self
            .jobs
            .iter()
            .map(|j| {
                j.spec
                    .inputs
                    .iter()
                    .map(|p| normalize(&self.cwd, p))
                    .collect()
            })
            .collect();
        let outputs: Vec<Vec<PathBuf>> = self
            .jobs
            .iter()
            .map(|j| {
                let mut effects: BTreeSet<PathBuf> = j
                    .spec
                    .outputs
                    .iter()
                    .map(|p| normalize(&self.cwd, p))
                    .collect();
                if j.spec.creates_parents {
                    // A job may create ancestors of its declared outputs.
                    // Include them even if present: earlier barriers may remove them.
                    let directories: Vec<_> = effects.iter().cloned().collect();
                    for directory in directories {
                        for parent in directory.ancestors().skip(1) {
                            if parent == self.cwd || parent.parent().is_none() {
                                break;
                            }
                            effects.insert(parent.to_owned());
                        }
                    }
                }
                effects.into_iter().collect()
            })
            .collect();
        let mut required_pairs = BTreeSet::new();
        let mut required = Vec::with_capacity(self.required_dependencies.len());
        let mut final_reads = std::collections::BTreeMap::<usize, BTreeSet<PathBuf>>::new();
        for (index, dependency) in self.required_dependencies.iter().enumerate() {
            let producer = self.node_for_source_line(dependency.producer_line)?;
            let consumer = self.node_for_source_line(dependency.consumer_line)?;
            if producer == consumer {
                bail!(
                    "required dependency resolves to the same executable node: source line {}",
                    dependency.consumer_line
                );
            }
            let artifact = normalize(&self.cwd, &dependency.artifact);
            if !outputs[producer].contains(&artifact) {
                bail!(
                    "required producer line {} does not declare output {}",
                    dependency.producer_line,
                    artifact.display()
                );
            }
            required_pairs.insert((producer, consumer));
            final_reads.entry(consumer).or_default().insert(artifact);
            required.push((producer, consumer, index));
        }
        let mut final_versions = std::collections::BTreeMap::<PathBuf, u32>::new();
        for (&consumer, artifacts) in &final_reads {
            for artifact in artifacts {
                if !final_versions.contains_key(artifact) {
                    final_versions.insert(artifact.clone(), 0);
                }
                for (producer, paths) in outputs.iter().enumerate() {
                    if paths.contains(artifact) && !required_pairs.contains(&(producer, consumer)) {
                        bail!(
                            "final-artifact dependency {} is missing producer line {} for consumer line {}",
                            artifact.display(),
                            self.jobs[producer].source.line,
                            self.jobs[consumer].source.line
                        );
                    }
                }
            }
        }
        if !final_versions.is_empty() {
            for path in outputs.iter().flatten() {
                if let Some(version) = final_versions.get_mut(path) {
                    *version += 1;
                }
            }
        }
        // Different hardlink names are aliases that lexical paths cannot expose.
        // Keep any such operation behind a barrier instead of guessing independence.
        let aliased: Vec<bool> = inputs
            .iter()
            .zip(&outputs)
            .map(|(reads, writes)| {
                reads.iter().chain(writes).any(|path| {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::MetadataExt;
                        std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.nlink() > 1)
                    }
                    #[cfg(not(unix))]
                    {
                        let _ = path;
                        false
                    }
                })
            })
            .collect();
        for i in 0..self.jobs.len() {
            self.reads.push(
                inputs[i]
                    .iter()
                    .map(|p| ArtifactId {
                        path: p.clone(),
                        version: if final_reads.get(&i).is_some_and(|paths| paths.contains(p)) {
                            *final_versions.get(p).unwrap_or(&0)
                        } else {
                            *versions.get(p).unwrap_or(&0)
                        },
                    })
                    .collect(),
            );
            self.writes.push(
                outputs[i]
                    .iter()
                    .map(|p| {
                        let v = versions.entry(p.clone()).or_default();
                        *v += 1;
                        ArtifactId {
                            path: p.clone(),
                            version: *v,
                        }
                    })
                    .collect(),
            );
            for prior in 0..i {
                // Required generated inputs intentionally read the generated
                // final version, rather than an older file at this log position.
                // Other data hazards are never overridden by a policy.
                let reverse_prerequisite = required_pairs.contains(&(i, prior))
                    && !self.jobs[i].spec.unknown_effects
                    && !aliased[i]
                    && !aliased[prior];
                let reason = if outputs[prior]
                    .iter()
                    .any(|a| inputs[i].iter().any(|b| overlaps(a, b)))
                {
                    Some("read after write / directory prerequisite")
                } else if outputs[prior]
                    .iter()
                    .any(|a| outputs[i].iter().any(|b| overlaps(a, b)))
                {
                    Some("write after write")
                } else if inputs[prior]
                    .iter()
                    .filter(|path| {
                        !final_reads
                            .get(&prior)
                            .is_some_and(|paths| paths.contains(*path))
                    })
                    .any(|a| outputs[i].iter().any(|b| overlaps(a, b)))
                {
                    Some("write after read")
                } else if !reverse_prerequisite
                    && (self.jobs[i].spec.unknown_effects
                        || self.jobs[prior].spec.unknown_effects
                        || aliased[i]
                        || aliased[prior])
                {
                    Some("unknown-effect or filesystem-alias ordering barrier")
                } else {
                    None
                };
                if let Some(reason) = reason {
                    self.edges.push(Edge {
                        from: prior,
                        to: i,
                        reason: reason.into(),
                    });
                }
            }
        }
        for (producer, consumer, index) in required {
            let reason = &self.required_dependencies[index].reason;
            if let Some(edge) = self
                .edges
                .iter_mut()
                .find(|e| e.from == producer && e.to == consumer)
            {
                if !edge.reason.split("; ").any(|part| part == reason.as_str()) {
                    edge.reason.push_str("; ");
                    edge.reason.push_str(reason);
                }
            } else {
                self.edges.push(Edge {
                    from: producer,
                    to: consumer,
                    reason: reason.clone(),
                });
            }
        }
        self.waves = self.topological_waves()?;
        Ok(())
    }
    fn node_for_source_line(&self, line: usize) -> Result<usize> {
        let mut nodes = self
            .provenance
            .iter()
            .enumerate()
            .filter(|(_, originals)| originals.iter().any(|job| job.source.line == line))
            .map(|(index, _)| index);
        let Some(node) = nodes.next() else {
            bail!("required dependency source line {line} is absent from graph provenance");
        };
        if nodes.next().is_some() {
            bail!("required dependency source line {line} is ambiguous");
        }
        Ok(node)
    }

    pub fn topological_waves(&self) -> Result<Vec<Vec<usize>>> {
        let mut degree = vec![0; self.jobs.len()];
        let mut next = vec![vec![]; self.jobs.len()];
        let mut unique = BTreeSet::new();
        for edge in &self.edges {
            if edge.from >= degree.len() || edge.to >= degree.len() {
                bail!("dependency edge has invalid node index");
            }
            if unique.insert((edge.from, edge.to)) {
                degree[edge.to] += 1;
                next[edge.from].push(edge.to);
            }
        }
        let mut ready: Vec<usize> = degree
            .iter()
            .enumerate()
            .filter_map(|(i, &n)| (n == 0).then_some(i))
            .collect();
        let mut waves = Vec::new();
        let mut visited = 0;
        while !ready.is_empty() {
            visited += ready.len();
            let mut following = BTreeSet::new();
            for &i in &ready {
                for &child in &next[i] {
                    degree[child] -= 1;
                    if degree[child] == 0 {
                        following.insert(child);
                    }
                }
            }
            waves.push(ready);
            ready = following.into_iter().collect();
        }
        if visited != self.jobs.len() {
            let blocked: Vec<_> = degree
                .iter()
                .enumerate()
                .filter(|(_, d)| **d > 0)
                .map(|(i, _)| {
                    format!(
                        "node {i}, line {}: {}",
                        self.jobs[i].source.line, self.jobs[i].source.text
                    )
                })
                .collect();
            bail!("dependency cycle: {}", blocked.join("; "));
        }
        Ok(waves)
    }
    pub fn dot(&self) -> String {
        let mut out = String::from("digraph rusty_make {\n");
        for (i, j) in self.jobs.iter().enumerate() {
            let label = format!("{i}: {} [{:?}]\n{}", j.spec.kind, j.strategy, j.source.text);
            out.push_str(&format!(
                "  n{i} [label={}];\n",
                serde_json::to_string(&label).unwrap()
            ));
        }
        for edge in &self.edges {
            out.push_str(&format!(
                "  n{} -> n{} [label={}];\n",
                edge.from,
                edge.to,
                serde_json::to_string(&edge.reason).unwrap()
            ));
        }
        out.push_str("}\n");
        out
    }
}
