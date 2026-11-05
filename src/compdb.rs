//! Compilation database export with a graph-aware, pre-serialization hook.
use crate::graph::BuildGraph;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, fs, path::Path};

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

fn windows_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    (bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\'))
        || path.starts_with(r"\\")
}

/// Format Windows paths lexically, without interpreting them using the host's
/// filesystem. A POSIX backslash is a filename character, not a separator.
fn export_path(path: &str, windows_directory: bool) -> Cow<'_, str> {
    if let Some(rest) = path.strip_prefix(r"\\?\") {
        if rest
            .get(..4)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"UNC\"))
        {
            return Cow::Owned(format!("//{}", rest[4..].replace('\\', "/")));
        }
        if windows_path(rest) {
            return Cow::Owned(rest.replace('\\', "/"));
        }
        // Other device namespaces have no ordinary drive/UNC equivalent.
        return Cow::Borrowed(path);
    }
    if path.contains('\\') && (windows_path(path) || (windows_directory && !path.starts_with('/')))
    {
        Cow::Owned(path.replace('\\', "/"))
    } else {
        Cow::Borrowed(path)
    }
}

fn format_path(path: &mut String, windows_directory: bool) {
    if let Cow::Owned(formatted) = export_path(path, windows_directory) {
        *path = formatted;
    }
}

fn format_arguments(arguments: &mut [String], windows_directory: bool) {
    let mut path_operand = false;
    let mut literal_operand = false;
    let mut positional_only = false;
    let mut opaque_operands = false;
    for (index, arg) in arguments.iter_mut().enumerate() {
        if index == 0 || path_operand {
            format_path(arg, windows_directory);
            path_operand = false;
            continue;
        }
        if literal_operand {
            literal_operand = false;
            continue;
        }
        if !positional_only && arg == "--" {
            positional_only = true;
            opaque_operands = false;
        } else if !positional_only
            && matches!(
                arg.as_str(),
                "-o" | "-I"
                    | "-L"
                    | "-B"
                    | "-isystem"
                    | "-iquote"
                    | "-idirafter"
                    | "-include"
                    | "-imacros"
                    | "-isysroot"
                    | "--sysroot"
                    | "-MF"
                    | "-MJ"
            )
        {
            path_operand = true;
            opaque_operands = false;
        } else if !positional_only && matches!(arg.as_str(), "-D" | "-U" | "-x" | "-MT" | "-MQ") {
            literal_operand = true;
            opaque_operands = false;
        } else if !positional_only
            && let Some(prefix) = [
                "--sysroot=",
                "-isystem",
                "-iquote",
                "-idirafter",
                "-isysroot",
                "-I",
                "-L",
                "-B",
                "-o",
                "-MF",
                "-MJ",
            ]
            .into_iter()
            .find(|prefix| arg.starts_with(prefix) && arg.len() > prefix.len())
        {
            let value = export_path(&arg[prefix.len()..], windows_directory);
            if let Cow::Owned(value) = value {
                *arg = format!("{prefix}{value}");
            }
            opaque_operands = false;
        } else if !positional_only && arg.starts_with('-') {
            // Unknown options can consume arbitrary strings, including things
            // that look like paths. Never rewrite those operands.
            opaque_operands = !matches!(arg.as_str(), "-c" | "-S" | "-E" | "-g")
                && !arg.starts_with("-D")
                && !arg.starts_with("-U")
                && !matches!(
                    arg.as_str(),
                    "-O" | "-O0" | "-O1" | "-O2" | "-O3" | "-Os" | "-Oz" | "-Og" | "-Ofast"
                );
        } else if !opaque_operands {
            format_path(arg, windows_directory);
        }
    }
}

impl CompileCommand {
    fn format_paths(&mut self) {
        let windows_directory = windows_path(&self.directory);
        format_path(&mut self.directory, windows_directory);
        format_path(&mut self.file, windows_directory);
        if let Some(output) = &mut self.output {
            format_path(output, windows_directory);
        }
        format_arguments(&mut self.arguments, windows_directory);
    }
}
impl CompilationDatabase {
    /// Export compiler provenance without modifying the graph or its original
    /// argv. Windows drive and UNC paths use ordinary forward-slash spelling;
    /// POSIX backslashes and non-path compiler arguments retain their meaning.
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
                let mut command = CompileCommand {
                    directory: graph.cwd.to_string_lossy().into_owned(),
                    file,
                    arguments: argv.clone(),
                    output: job
                        .spec
                        .parsed
                        .get("output")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned),
                };
                command.format_paths();
                commands.push(command);
            }
        }
        Ok(Self { commands })
    }
    /// The transform can inspect dependencies, filter entries or rewrite argv,
    /// directory, file and output. Failure returns before touching the file.
    /// This is an API hook, not execution of arbitrary user code from CLI input.
    /// Paths introduced by the hook are formatted only in the serialization
    /// copy, so the hook's in-memory edits remain available to its caller.
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
        // The hook may relocate entries using native Windows paths. Format a
        // serialization copy, preserving the hook's in-memory values.
        let mut commands = self.commands.clone();
        for command in &mut commands {
            command.format_paths();
        }
        let bytes = serde_json::to_vec_pretty(&commands)?;
        fs::write(path, bytes)
            .with_context(|| format!("write compilation database {}", path.display()))
    }
    pub fn save(&mut self, graph: &BuildGraph, path: &Path) -> Result<()> {
        self.save_with_transform(graph, path, |_, _| Ok(()))
    }
}
