use crate::{
    commands::compiler::Compiler,
    graph::components,
    job::{FileAccess, Job, Producer},
};
use anyhow::{Context, Result, bail};
use std::{
    any::Any,
    collections::{BTreeMap, BTreeSet},
    fs,
    io::ErrorKind,
};

#[derive(Debug, Default)]
struct Node {
    children: BTreeMap<String, Node>,
    header: Option<String>,
}

#[derive(Debug, Default)]
pub struct HeaderTrie {
    root: Node,
}

impl HeaderTrie {
    pub fn insert(&mut self, path: impl Into<String>) {
        let path = path.into();
        let mut node = &mut self.root;
        for component in components(&path) {
            node = node.children.entry(component.to_owned()).or_default();
        }
        node.header = Some(path);
    }

    pub fn headers_under(&self, directory: &str) -> Vec<&str> {
        let mut node = &self.root;
        for component in components(directory) {
            let Some(child) = node.children.get(component) else {
                return Vec::new();
            };
            node = child;
        }
        let mut result = Vec::new();
        let mut pending = vec![node];
        while let Some(node) = pending.pop() {
            if let Some(header) = &node.header {
                result.push(header.as_str());
            }
            pending.extend(node.children.values().rev());
        }
        result
    }
}

fn is_header(path: &str) -> bool {
    let Some((_, extension)) = path.rsplit('/').next().unwrap_or(path).rsplit_once('.') else {
        return false;
    };
    [
        "h", "hh", "hp", "hpp", "hxx", "h++", "inc", "inl", "ipp", "tpp", "tcc", "cuh",
    ]
    .iter()
    .any(|candidate| extension.eq_ignore_ascii_case(candidate))
}

/// Inventory existing include-directory headers without resolving or rewriting
/// their spellings. Symlinked child directories are never followed.
pub fn discover_headers(jobs: &[Box<dyn Job>]) -> Result<Vec<String>> {
    let roots: BTreeSet<_> = jobs
        .iter()
        .filter_map(|job| (job.as_ref() as &dyn Any).downcast_ref::<Compiler>())
        .flat_map(|compiler| compiler.include_dirs.iter().cloned())
        .collect();
    let mut pending: Vec<_> = roots.into_iter().collect();
    let mut visited = BTreeSet::new();
    let mut headers = BTreeSet::new();
    while let Some(directory) = pending.pop() {
        if !visited.insert(directory.clone()) {
            continue;
        }
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("reading header directory {directory:?}"));
            }
        };
        for entry in entries {
            let entry = entry?;
            let filename = entry
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("non-UTF-8 header entry in {directory:?}"))?;
            let path = if directory.ends_with('/') {
                format!("{directory}{filename}")
            } else {
                format!("{directory}/{filename}")
            };
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(path);
            } else if (kind.is_file() || kind.is_symlink()) && is_header(&path) {
                headers.insert(path);
            }
        }
    }
    Ok(headers.into_iter().collect())
}

/// Conservatively read every known header beneath each compiler include dir.
/// Existing preceding writers select the current value; only a unique future
/// writer can supply a header with no preceding writer.
pub fn add_header_dependencies(
    jobs: &mut [Box<dyn Job>],
    existing_headers: impl IntoIterator<Item = String>,
) -> Result<()> {
    let mut trie = HeaderTrie::default();
    for header in existing_headers {
        if is_header(&header) {
            trie.insert(header);
        }
    }
    let mut writers: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (index, job) in jobs.iter().enumerate() {
        for access in &job.data().accesses {
            if access.access.writes() && is_header(&access.path) {
                trie.insert(access.path.clone());
                let entries = writers.entry(&access.path).or_default();
                if entries.last() != Some(&index) {
                    entries.push(index);
                }
            }
        }
    }
    let mut additions = Vec::new();
    for (consumer, job) in jobs.iter().enumerate() {
        let Some(compiler) = (job.as_ref() as &dyn Any).downcast_ref::<Compiler>() else {
            continue;
        };
        let headers: BTreeSet<_> = compiler
            .include_dirs
            .iter()
            .flat_map(|directory| trie.headers_under(directory))
            .collect();
        let mut accesses = Vec::new();
        let mut producers = Vec::new();
        for header in headers {
            if job
                .data()
                .accesses
                .iter()
                .any(|access| access.access.writes() && access.path == header)
            {
                continue;
            }
            if !job
                .data()
                .accesses
                .iter()
                .any(|access| access.access.reads() && access.path == header)
            {
                accesses.push(FileAccess::read(header));
            }
            if job
                .data()
                .producers
                .iter()
                .any(|producer| producer.input == header)
            {
                continue;
            }
            let Some(candidates) = writers.get(header) else {
                continue;
            };
            let preceding = candidates.partition_point(|&writer| writer < consumer);
            let producer = if preceding != 0 {
                Some(candidates[preceding - 1])
            } else {
                let mut future = candidates
                    .iter()
                    .copied()
                    .filter(|&writer| writer > consumer);
                let first = future.next();
                if future.next().is_some() {
                    bail!("ambiguous future producers for header {header:?} at job {consumer}");
                }
                first
            };
            if let Some(job) = producer {
                producers.push(Producer {
                    input: header.to_owned(),
                    job,
                });
            }
        }
        additions.push((consumer, accesses, producers));
    }
    // Validation above is transactional: ambiguity does not partially edit jobs.
    for (consumer, accesses, producers) in additions {
        jobs[consumer].data_mut().accesses.extend(accesses);
        jobs[consumer].data_mut().producers.extend(producers);
    }
    Ok(())
}
