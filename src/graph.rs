use crate::job::Job;
use anyhow::{Context, Result, bail};
use std::{
    any::Any,
    collections::{BTreeMap, BTreeSet, HashMap},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
}

#[derive(Debug)]
pub struct Dag {
    pub jobs: Vec<Box<dyn Job>>,
    pub edges: Vec<Edge>,
    pub waves: Vec<Vec<usize>>,
    pub unproduced: Vec<Vec<String>>,
}

#[derive(Default)]
struct PathNode {
    children: BTreeMap<String, usize>,
    parent: Option<usize>,
    writers: Vec<usize>,
}

// Forward slashes delimit components; a directory's trailing slash adds no
// component. Original strings remain untouched.
pub(crate) fn components(path: &str) -> impl Iterator<Item = &str> {
    path.split_terminator('/')
}

#[derive(Default)]
struct Paths {
    nodes: Vec<PathNode>,
    ids: HashMap<String, usize>,
}

impl Paths {
    fn insert(&mut self, path: &str) -> usize {
        if let Some(&id) = self.ids.get(path) {
            return id;
        }
        if self.nodes.is_empty() {
            self.nodes.push(PathNode::default());
        }
        let mut node = 0;
        for part in components(path) {
            node = if let Some(&child) = self.nodes[node].children.get(part) {
                child
            } else {
                let child = self.nodes.len();
                self.nodes.push(PathNode {
                    parent: Some(node),
                    ..Default::default()
                });
                self.nodes[node].children.insert(part.to_owned(), child);
                child
            };
        }
        self.ids.insert(path.to_owned(), node);
        node
    }

    fn overlapping(&self, node: usize) -> Vec<usize> {
        let mut result = Vec::new();
        let mut parent = self.nodes[node].parent;
        while let Some(id) = parent {
            if !self.nodes[id].writers.is_empty() {
                result.push(id);
            }
            parent = self.nodes[id].parent;
        }
        let mut pending = vec![node];
        while let Some(id) = pending.pop() {
            if !self.nodes[id].writers.is_empty() {
                result.push(id);
            }
            pending.extend(self.nodes[id].children.values().copied());
        }
        result
    }
}

pub fn create_graph(jobs: Vec<Box<dyn Job>>) -> Result<Dag> {
    let mut dag = Dag {
        jobs,
        edges: Vec::new(),
        waves: Vec::new(),
        unproduced: Vec::new(),
    };
    dag.rebuild()?;
    Ok(dag)
}

impl Dag {
    pub fn rebuild(&mut self) -> Result<()> {
        let mut paths = Paths::default();
        for (index, job) in self.jobs.iter().enumerate() {
            for access in &job.data().accesses {
                let id = paths.insert(&access.path);
                if access.access.writes() && paths.nodes[id].writers.last() != Some(&index) {
                    paths.nodes[id].writers.push(index);
                }
            }
        }
        // Resolve matching writer nodes once per spelling, not once per access.
        let overlaps: HashMap<_, _> = paths
            .ids
            .values()
            .copied()
            .map(|id| (id, paths.overlapping(id)))
            .collect();
        let mut edges = BTreeSet::new();
        let mut unproduced = vec![Vec::new(); self.jobs.len()];
        let mut bindings = Vec::with_capacity(self.jobs.len());
        let mut producer_pairs = BTreeSet::new();
        for (consumer, job) in self.jobs.iter().enumerate() {
            let mut bound = HashMap::new();
            for producer in &job.data().producers {
                if producer.job >= self.jobs.len() || producer.job == consumer {
                    bail!("invalid producer {} for job {consumer}", producer.job);
                }
                if !job
                    .data()
                    .accesses
                    .iter()
                    .any(|a| a.access.reads() && a.path == producer.input)
                {
                    bail!(
                        "producer binding for unread input {:?} at job {consumer}",
                        producer.input
                    );
                }
                let id = paths.ids[&producer.input];
                if !overlaps[&id].iter().any(|&node| {
                    paths.nodes[node]
                        .writers
                        .binary_search(&producer.job)
                        .is_ok()
                }) {
                    bail!("job {} does not produce {:?}", producer.job, producer.input);
                }
                if let Some(previous) = bound.insert(producer.input.as_str(), producer.job) {
                    if previous != producer.job {
                        bail!(
                            "multiple producers bound to {:?} at job {consumer}",
                            producer.input
                        );
                    }
                }
                edges.insert((producer.job, consumer));
                producer_pairs.insert((producer.job, consumer));
            }
            bindings.push(bound);
        }
        for (index, job) in self.jobs.iter().enumerate() {
            for access in &job.data().accesses {
                let id = paths.ids[&access.path];
                let matching = &overlaps[&id];
                if access.access.writes() {
                    for &node in matching {
                        let writers = &paths.nodes[node].writers;
                        let before = writers.partition_point(|&writer| writer < index);
                        if before != 0 {
                            edges.insert((writers[before - 1], index));
                        }
                    }
                }
                if !access.access.reads() {
                    continue;
                }
                let explicit = bindings[index].get(access.path.as_str()).copied();
                let mut produced = explicit.is_some();
                let boundary = explicit.unwrap_or(index);
                for &node in matching {
                    let writers = &paths.nodes[node].writers;
                    if explicit.is_none() {
                        let before = writers.partition_point(|&writer| writer < index);
                        if before != 0 {
                            edges.insert((writers[before - 1], index));
                            produced |= node == id;
                        }
                    }
                    let after = writers.partition_point(|&writer| writer <= boundary);
                    // A read-modify-write consumes its incoming value, not itself.
                    if let Some(&next) = writers[after..].iter().find(|&&writer| writer != index) {
                        edges.insert((index, next));
                    }
                }
                if !produced && !unproduced[index].contains(&access.path) {
                    unproduced[index].push(access.path.clone());
                }
            }
        }
        // Reversing two barriers breaks the ordinary frontier's transitivity.
        // Retain every original constraint touching those exceptional barriers.
        let mut reversed_barriers = BTreeSet::new();
        for &(producer, consumer) in &producer_pairs {
            if producer > consumer
                && self.jobs[producer].data().unknown_effects
                && self.jobs[consumer].data().unknown_effects
            {
                reversed_barriers.extend([producer, consumer]);
            }
        }
        let mut previous_barrier = None;
        let mut segment = Vec::new();
        let mut barrier_edge = |from, to| {
            if producer_pairs.contains(&(to, from)) {
                edges.insert((to, from));
            } else {
                edges.insert((from, to));
            }
        };
        for barrier in reversed_barriers {
            for other in 0..self.jobs.len() {
                if other != barrier {
                    barrier_edge(barrier.min(other), barrier.max(other));
                }
            }
        }
        for (index, job) in self.jobs.iter().enumerate() {
            if job.data().unknown_effects {
                if let Some(previous) = previous_barrier {
                    barrier_edge(previous, index);
                }
                for before in segment.drain(..) {
                    barrier_edge(before, index);
                }
                previous_barrier = Some(index);
            } else {
                if let Some(previous) = previous_barrier {
                    barrier_edge(previous, index);
                }
                segment.push(index);
            }
        }
        let edges: Vec<_> = edges
            .into_iter()
            .map(|(from, to)| Edge { from, to })
            .collect();
        let waves = topological_waves(self.jobs.len(), &edges)?;
        self.edges = edges;
        self.waves = waves;
        self.unproduced = unproduced;
        Ok(())
    }

    pub fn check_inputs(&self) -> Result<()> {
        for (index, job) in self.jobs.iter().enumerate() {
            let inputs: Vec<_> = self.unproduced[index].iter().map(String::as_str).collect();
            job.check_inputs(&inputs)
                .with_context(|| format!("checking inputs for job {index} ({})", job.name()))?;
        }
        Ok(())
    }

    pub fn topological_waves(&self) -> Result<Vec<Vec<usize>>> {
        topological_waves(self.jobs.len(), &self.edges)
    }

    pub fn batches(&self) -> Vec<Vec<usize>> {
        self.batches_in(&self.waves)
    }

    pub(crate) fn batches_in(&self, waves: &[Vec<usize>]) -> Vec<Vec<usize>> {
        let mut batches = Vec::new();
        for wave in waves {
            let mut groups: Vec<(std::any::TypeId, Vec<usize>)> = Vec::new();
            for &index in wave {
                let kind = (self.jobs[index].as_ref() as &dyn Any).type_id();
                if let Some((_, group)) = groups.iter_mut().find(|(id, _)| *id == kind) {
                    group.push(index);
                } else {
                    groups.push((kind, vec![index]));
                }
            }
            batches.extend(groups.into_iter().map(|(_, group)| group));
        }
        batches
    }
}

pub(crate) fn topological_waves(count: usize, edges: &[Edge]) -> Result<Vec<Vec<usize>>> {
    let mut successors = vec![Vec::new(); count];
    let mut indegree = vec![0; count];
    for edge in edges {
        if edge.from >= count || edge.to >= count {
            bail!("edge endpoint outside graph: {} -> {}", edge.from, edge.to);
        }
        successors[edge.from].push(edge.to);
        indegree[edge.to] += 1;
    }
    let mut ready: Vec<_> = (0..count).filter(|&index| indegree[index] == 0).collect();
    let mut waves = Vec::new();
    let mut visited = 0;
    while !ready.is_empty() {
        ready.sort_unstable();
        visited += ready.len();
        let mut next = Vec::new();
        for &node in &ready {
            for &successor in &successors[node] {
                indegree[successor] -= 1;
                if indegree[successor] == 0 {
                    next.push(successor);
                }
            }
        }
        waves.push(ready);
        ready = next;
    }
    if visited != count {
        bail!("dependency cycle in job graph");
    }
    Ok(waves)
}
