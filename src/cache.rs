use crate::{graph::BuildGraph, job::JobRecord};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

const SCHEMA: &str = "rusty-make-trait-jobs-v2";
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct Record {
    fingerprint: String,
    inputs: Vec<(PathBuf, String)>,
    outputs: Vec<(PathBuf, String)>,
}
#[derive(Default, Serialize, Deserialize)]
struct Store {
    records: BTreeMap<String, Record>,
}
pub struct Cache {
    path: PathBuf,
    store: Store,
    environment: String,
    changed: bool,
}

/// No metadata-only hash reuse. Symlinks and multi-link regular files cannot
/// validate this narrow native cache and always miss.
fn digest(path: &Path) -> Result<Option<String>> {
    for ancestor in path.ancestors() {
        if let Ok(meta) = fs::symlink_metadata(ancestor) {
            if meta.file_type().is_symlink() {
                return Ok(None);
            }
        }
    }
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut hash = blake3::Hasher::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.is_file() && meta.nlink() != 1 {
            return Ok(None);
        }
        hash.update(&meta.mode().to_le_bytes());
    }
    if meta.is_dir() {
        hash.update(b"directory-exists");
    } else if meta.is_file() {
        let mut file = fs::File::open(path)?;
        let mut buffer = [0u8; 65536];
        loop {
            let n = file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            hash.update(&buffer[..n]);
        }
    } else {
        return Ok(None);
    }
    Ok(Some(hash.finalize().to_hex().to_string()))
}
fn snapshots(paths: &[PathBuf], cwd: &Path) -> Result<Option<Vec<(PathBuf, String)>>> {
    let mut values = vec![];
    for p in paths {
        let path = if p.is_absolute() {
            p.clone()
        } else {
            cwd.join(p)
        };
        let Some(hash) = digest(&path)? else {
            return Ok(None);
        };
        values.push((p.clone(), hash));
    }
    Ok(Some(values))
}
impl Cache {
    pub fn open(cwd: &Path) -> Result<Self> {
        let path = cwd.join(".rusty_make/cache.json");
        let store = match fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("[CACHE] invalid cache ignored: {e}");
                    Store::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Store::default(),
            Err(e) => return Err(e).context("read cache"),
        };
        let env: BTreeMap<_, _> = std::env::vars_os()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().to_string(),
                    v.to_string_lossy().to_string(),
                )
            })
            .collect();
        let environment = blake3::hash(&serde_json::to_vec(&env)?)
            .to_hex()
            .to_string();
        Ok(Self {
            path,
            store,
            environment,
            changed: false,
        })
    }
    fn key(graph: &BuildGraph, i: usize) -> String {
        format!(
            "{}:{}",
            graph.jobs[i].source.line,
            graph.jobs[i].source.target.as_deref().unwrap_or("")
        )
    }
    fn fingerprint(&self, job: &JobRecord, cwd: &Path) -> Result<String> {
        Ok(blake3::hash(&serde_json::to_vec(&(
            SCHEMA,
            env!("CARGO_PKG_VERSION"),
            cwd,
            &self.environment,
            job,
        ))?)
        .to_hex()
        .to_string())
    }
    pub fn check(&self, graph: &BuildGraph, i: usize) -> Result<std::result::Result<(), String>> {
        let job = &graph.jobs[i];
        if job.spec.unknown_effects {
            return Ok(Err(
                "filesystem effects cannot be completely validated".into()
            ));
        }
        if !job.spec.cacheable {
            return Ok(Err("job is not incrementally cacheable".into()));
        }
        let Some(old) = self.store.records.get(&Self::key(graph, i)) else {
            return Ok(Err("no successful cache record".into()));
        };
        if old.fingerprint != self.fingerprint(job, &graph.cwd)? {
            return Ok(Err(
                "command, environment, or execution configuration changed".into(),
            ));
        }
        let Some(inputs) = snapshots(&job.spec.inputs, &graph.cwd)? else {
            return Ok(Err(
                "input missing or unsafe to hash (symlink/alias/special file)".into(),
            ));
        };
        if old.inputs != inputs {
            return Ok(Err("input content or permissions changed".into()));
        }
        let Some(outputs) = snapshots(&job.spec.outputs, &graph.cwd)? else {
            return Ok(Err("output missing or unsafe to hash".into()));
        };
        if old.outputs != outputs {
            return Ok(Err("output content or permissions changed".into()));
        }
        Ok(Ok(()))
    }
    /// Invalidate before launching, including prior successful entries. A failure
    /// must never leave an old record that could validate a partially run job.
    pub fn invalidate(&mut self, graph: &BuildGraph, i: usize) {
        if self.store.records.remove(&Self::key(graph, i)).is_some() {
            self.changed = true;
        }
    }
    pub fn record(&mut self, graph: &BuildGraph, i: usize) -> Result<()> {
        let job = &graph.jobs[i];
        if !job.spec.cacheable || job.spec.unknown_effects {
            return Ok(());
        }
        let (Some(inputs), Some(outputs)) = (
            snapshots(&job.spec.inputs, &graph.cwd)?,
            snapshots(&job.spec.outputs, &graph.cwd)?,
        ) else {
            return Ok(());
        };
        let record = Record {
            fingerprint: self.fingerprint(job, &graph.cwd)?,
            inputs,
            outputs,
        };
        self.store.records.insert(Self::key(graph, i), record);
        self.changed = true;
        Ok(())
    }
    pub fn save(&mut self) -> Result<()> {
        if !self.changed {
            return Ok(());
        }
        let parent = self.path.parent().unwrap();
        fs::create_dir_all(parent)?;
        // Single writer per build root. create_new refuses concurrent writers;
        // the temporary file is never a valid cache entry after a crash.
        let tmp = parent.join("cache.json.tmp");
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&tmp)
                .context("cache is locked or stale .rusty_make/cache.json.tmp exists")?;
            file.write_all(&serde_json::to_vec_pretty(&self.store)?)?;
            file.sync_all()?;
            fs::rename(&tmp, &self.path)?;
            Ok(())
        })();
        if result.is_ok() {
            self.changed = false;
        }
        result
    }
}
