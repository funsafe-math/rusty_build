//! Compilation database export with a graph-aware, pre-serialization hook.
use crate::graph::BuildGraph;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompileCommand {
    pub directory: String,
    pub file: String,
    pub arguments: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}
#[derive(Clone, Debug, Default)]
pub struct CompilationDatabase {
    pub commands: Vec<CompileCommand>,
}
impl CompilationDatabase {
    pub fn from_graph(graph: &BuildGraph) -> Result<Self> {
        let mut commands = vec![];
        for job in graph.provenance.iter().flatten() {
            if !matches!(job.spec.kind.as_str(), "compile" | "link") {
                continue;
            }
            let Some(sources) = job.spec.parsed.get("sources").and_then(|v| v.as_array()) else {
                continue;
            };
            let argv: Vec<String> = serde_json::from_value(job.spec.parsed["argv"].clone())
                .context("compiler argv metadata")?;
            for source in sources {
                let file = source
                    .as_str()
                    .context("compiler source metadata must be a path string")?
                    .to_owned();
                commands.push(CompileCommand {
                    directory: graph.cwd.to_string_lossy().into_owned(),
                    file,
                    arguments: argv.clone(),
                    output: job
                        .spec
                        .parsed
                        .get("output")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned),
                });
            }
        }
        Ok(Self { commands })
    }
    /// The transform can inspect dependencies, filter entries or rewrite argv,
    /// directory, file and output. Failure returns before touching the file.
    /// This is an API hook, not execution of arbitrary user code from CLI input.
    pub fn save_with_transform<F>(
        &mut self,
        graph: &BuildGraph,
        path: &Path,
        transform: F,
    ) -> Result<()>
    where
        F: FnOnce(&BuildGraph, &mut Self) -> Result<()>,
    {
        transform(graph, self)?;
        let bytes = serde_json::to_vec_pretty(&self.commands)?;
        fs::write(path, bytes)
            .with_context(|| format!("write compilation database {}", path.display()))
    }
    pub fn save(&mut self, graph: &BuildGraph, path: &Path) -> Result<()> {
        self.save_with_transform(graph, path, |_, _| Ok(()))
    }
}
