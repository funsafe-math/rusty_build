use crate::job::{JobRecord, JobSnapshot};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ffi::OsString,
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
    normalize_with(cwd, path, |path| path.canonicalize().ok())
}

fn normalize_with(
    cwd: &Path,
    path: &Path,
    mut canonicalize: impl FnMut(&Path) -> Option<PathBuf>,
) -> PathBuf {
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
                if let Some(existing) = canonicalize(&resolved) {
                    resolved = existing;
                }
            }
        }
    }
    resolved
}
/// Filesystem observations belong to one rebuild, including failed lookups:
/// independent outputs usually share most of their existing ancestors.
#[derive(Default)]
struct PathCache {
    normalized: HashMap<PathBuf, PathBuf>,
    canonical: HashMap<PathBuf, Option<PathBuf>>,
    aliases: HashMap<PathBuf, bool>,
}

impl PathCache {
    fn normalize(&mut self, cwd: &Path, path: &Path) -> PathBuf {
        if let Some(normalized) = self.normalized.get(path) {
            return normalized.clone();
        }
        let normalized = normalize_with(cwd, path, |prefix| {
            if let Some(canonical) = self.canonical.get(prefix) {
                return canonical.clone();
            }
            let canonical = prefix.canonicalize().ok();
            self.canonical.insert(prefix.to_owned(), canonical.clone());
            canonical
        });
        self.normalized.insert(path.to_owned(), normalized.clone());
        normalized
    }

    fn aliased(&mut self, path: &Path) -> bool {
        if let Some(&aliased) = self.aliases.get(path) {
            return aliased;
        }
        let aliased = {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.nlink() > 1)
            }
            #[cfg(not(unix))]
            {
                false
            }
        };
        self.aliases.insert(path.to_owned(), aliased);
        aliased
    }
}

/// Exact-path users live at trie nodes. A query visits only its ancestors and
/// descendants, never sibling paths, and collects jobs deterministically.
#[derive(Default)]
struct HazardIndex {
    users: BTreeSet<usize>,
    children: HashMap<OsString, HazardIndex>,
}

impl HazardIndex {
    fn insert(&mut self, path: &Path, job: usize) {
        let mut node = self;
        for component in path.components() {
            let key = component.as_os_str();
            if !node.children.contains_key(key) {
                node.children.insert(key.to_owned(), Self::default());
            }
            node = node.children.get_mut(key).expect("inserted hazard path");
        }
        node.users.insert(job);
    }

    fn overlapping(
        &self,
        path: &Path,
        predecessors: &mut BTreeMap<usize, &'static str>,
        reason: &'static str,
    ) {
        let mut node = self;
        for component in path.components() {
            for &job in &node.users {
                predecessors.entry(job).or_insert(reason);
            }
            let Some(child) = node.children.get(component.as_os_str()) else {
                return;
            };
            node = child;
        }
        node.collect(predecessors, reason);
    }

    fn collect(&self, predecessors: &mut BTreeMap<usize, &'static str>, reason: &'static str) {
        for &job in &self.users {
            predecessors.entry(job).or_insert(reason);
        }
        for child in self.children.values() {
            child.collect(predecessors, reason);
        }
    }
}

/// Execution paths must accept ordinary forward-slash recipe operands.
/// Windows canonicalize produces a verbatim prefix with different join rules;
/// reserve canonicalization for hazard identities, not the execution cwd.
fn execution_cwd(cwd: &Path) -> Result<PathBuf> {
    #[cfg(windows)]
    {
        use std::path::Prefix;
        let absolute = std::path::absolute(cwd).context("resolve build working directory")?;
        if !absolute
            .metadata()
            .context("inspect build working directory")?
            .is_dir()
        {
            bail!(
                "build working directory is not a directory: {}",
                absolute.display()
            );
        }
        let mut components = absolute.components();
        // Older saved plans already contain canonicalize's verbatim spelling.
        // Convert only those typed prefixes, never arbitrary path characters.
        let mut ordinary = match components.next() {
            Some(Component::Prefix(prefix)) => match prefix.kind() {
                Prefix::VerbatimDisk(drive) => PathBuf::from(format!("{}:", char::from(drive))),
                Prefix::VerbatimUNC(server, share) => Path::new(r"\\").join(server).join(share),
                _ => return Ok(absolute),
            },
            _ => return Ok(absolute),
        };
        ordinary.extend(components);
        Ok(ordinary)
    }
    #[cfg(not(windows))]
    {
        cwd.canonicalize()
            .context("resolve build working directory")
    }
}

impl BuildGraph {
    pub fn new(jobs: Vec<JobRecord>, cwd: &Path) -> Result<Self> {
        let cwd = execution_cwd(cwd)?;
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
            cwd: execution_cwd(&stored.cwd)?,
            edges: vec![],
            required_dependencies: stored.required_dependencies,
            waves: vec![],
            reads: vec![],
            writes: vec![],
            decisions: stored.decisions,
            diagnostics: stored.diagnostics,
        };
        graph.rebuild()?;
        let mut edge_pairs: BTreeSet<_> = graph
            .edges
            .iter()
            .map(|edge| (edge.from, edge.to))
            .collect();
        for edge in stored.edges {
            if edge_pairs.insert((edge.from, edge.to)) {
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
        let mut paths = PathCache::default();
        let mut versions = std::collections::BTreeMap::<PathBuf, u32>::new();
        let inputs: Vec<Vec<PathBuf>> = self
            .jobs
            .iter()
            .map(|j| {
                j.spec
                    .inputs
                    .iter()
                    .map(|p| paths.normalize(&self.cwd, p))
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
                    .map(|p| paths.normalize(&self.cwd, p))
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
            let artifact = paths.normalize(&self.cwd, &dependency.artifact);
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
            .map(|(reads, writes)| reads.iter().chain(writes).any(|path| paths.aliased(path)))
            .collect();
        let barriers: Vec<bool> = self
            .jobs
            .iter()
            .zip(&aliased)
            .map(|(job, aliased)| job.spec.unknown_effects || *aliased)
            .collect();
        let mut writers = HazardIndex::default();
        let mut readers = HazardIndex::default();
        let mut frontier = BTreeSet::new();
        let mut last_barrier = None;
        let mut prior_barriers = Vec::new();
        let hoisting = required_pairs
            .iter()
            .any(|&(producer, consumer)| producer > consumer);
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
            let mut predecessors = BTreeMap::new();
            // Retain every actual hazard (and the original reason precedence),
            // even when its ordering is also implied by a barrier.
            for path in &inputs[i] {
                writers.overlapping(
                    path,
                    &mut predecessors,
                    "read after write / directory prerequisite",
                );
            }
            for path in &outputs[i] {
                writers.overlapping(path, &mut predecessors, "write after write");
            }
            for path in &outputs[i] {
                readers.overlapping(path, &mut predecessors, "write after read");
            }
            const BARRIER: &str = "unknown-effect or filesystem-alias ordering barrier";
            if hoisting {
                // A backward explicit requirement can remove one particular
                // consumer->producer barrier, but must not remove intervening
                // paths that reveal an unsafe hoist as a cycle. Keep the exact
                // barrier relation for this policy-only case. Path hazards are
                // still indexed; ordinary plans use the linear frontier below.
                let mut add_barrier = |prior| {
                    let reverse_prerequisite =
                        required_pairs.contains(&(i, prior)) && !barriers[i] && !aliased[prior];
                    if !reverse_prerequisite {
                        predecessors.entry(prior).or_insert(BARRIER);
                    }
                };
                if barriers[i] {
                    for prior in 0..i {
                        add_barrier(prior);
                    }
                } else {
                    for &prior in &prior_barriers {
                        add_barrier(prior);
                    }
                }
            } else if barriers[i] {
                // All earlier nodes reach a current sink. Joining those sinks
                // orders the entire prefix without a dense all-pairs barrier.
                for &prior in &frontier {
                    predecessors.entry(prior).or_insert(BARRIER);
                }
            } else if let Some(prior) = last_barrier {
                predecessors.entry(prior).or_insert(BARRIER);
            }
            for (prior, reason) in predecessors {
                if !hoisting {
                    frontier.remove(&prior);
                }
                self.edges.push(Edge {
                    from: prior,
                    to: i,
                    reason: reason.into(),
                });
            }
            if !hoisting {
                frontier.insert(i);
            }
            if barriers[i] {
                if hoisting {
                    prior_barriers.push(i);
                } else {
                    last_barrier = Some(i);
                }
            }
            // Final-artifact policy reads deliberately do not cause a WAR at
            // their original log position; all other reads do.
            for path in &inputs[i] {
                if !final_reads
                    .get(&i)
                    .is_some_and(|paths| paths.contains(path))
                {
                    readers.insert(path, i);
                }
            }
            for path in &outputs[i] {
                writers.insert(path, i);
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
