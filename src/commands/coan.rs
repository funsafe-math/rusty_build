use std::any::Any;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};

use anyhow::{Context, Result, bail};
use same_file::Handle;

use crate::job::{FileAccess, Job, JobData, is_null_device};
use crate::shell::ShellHints;

use super::external::{command_status, configure_stdio, finish};
use super::fs::{checked_regular, destination_metadata, reject_symlink_components};

/// Coan's source transformations have closed effects only with explicit files.
/// Everything else still executes the original external command unchanged.
#[derive(Debug)]
pub struct Coan {
    pub data: JobData,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    /// Executable, mode and options, without file operands (for batch matching).
    pub args: Vec<String>,
    pub argv: Vec<String>,
    pub replace: bool,
    batch_safe: bool,
}

impl Coan {
    pub fn parse(argv: &[String], shell: &ShellHints) -> Result<Self> {
        let mut job = Self {
            data: JobData::default(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            args: argv.iter().take(2).cloned().collect(),
            argv: argv.to_vec(),
            replace: false,
            batch_safe: true,
        };
        if argv.is_empty() {
            bail!("coan has no executable");
        }
        let source = argv.get(1).is_some_and(|mode| mode == "source");
        let mut understood = source;
        let mut operands_only = false;
        let mut index = 2;
        while index < argv.len() {
            let arg = &argv[index];
            if operands_only || !arg.starts_with('-') || arg == "-" {
                job.inputs.push(arg.clone());
                index += 1;
                continue;
            }
            job.args.push(arg.clone());
            match arg.as_str() {
                "--" => {
                    operands_only = true;
                    job.batch_safe = false;
                }
                "-r" | "--replace" => job.replace = true,
                "-m" | "--implicit" | "-V" | "--verbose" | "-K" | "--keepgoing"
                | "--no-transients" | "-P" | "--pod" | "-E" | "--eval-wip" | "-c"
                | "--complement" => {}
                "--line" => job.batch_safe = false,
                "-R" | "--recurse" => understood = false,
                _ => {
                    let long = arg.split_once('=').map_or(arg.as_str(), |(name, _)| name);
                    let long_value = matches!(
                        long,
                        "--define"
                            | "--undef"
                            | "--gag"
                            | "--discard"
                            | "--conflict"
                            | "--max-expansion"
                            | "--filter"
                            | "--backup"
                            | "--file"
                            | "--dir"
                            | "--prefix"
                            | "--select"
                    );
                    let short = (!arg.starts_with("--")).then(|| arg.as_bytes()[1]);
                    let short_value = short.is_some_and(|flag| {
                        matches!(
                            flag,
                            b'D' | b'U' | b'g' | b'k' | b'x' | b'F' | b'b' | b'f' | b'p'
                        )
                    });
                    if !long_value && !short_value {
                        understood = false;
                    }
                    if matches!(
                        long,
                        "--backup" | "--file" | "--dir" | "--prefix" | "--select"
                    ) || short.is_some_and(|flag| matches!(flag, b'b' | b'f' | b'p'))
                    {
                        understood = false;
                    }
                    // Filtering uses the original filename's extension, not the
                    // copied output's extension; --line embeds that name as well.
                    if long == "--filter" || short == Some(b'F') {
                        job.batch_safe = false;
                    }
                    if (long_value && !arg.contains('=')) || (short_value && arg.len() == 2) {
                        index += 1;
                        let value = argv
                            .get(index)
                            .with_context(|| format!("coan option {arg} requires a value"))?;
                        if value.is_empty() {
                            bail!("coan option {arg} requires a nonempty value");
                        }
                        job.args.push(value.clone());
                    } else if long_value && arg.ends_with('=') {
                        bail!("coan option {long} requires a nonempty value");
                    }
                }
            }
            index += 1;
        }
        let explicit_files = !job.inputs.is_empty() && !job.inputs.iter().any(|name| name == "-");
        let stdout_output = !job.replace
            && job.inputs.len() == 1
            && shell
                .stdout
                .as_deref()
                .is_some_and(|path| !is_null_device(path));
        let closed = understood && explicit_files && (job.replace || stdout_output);
        job.data.unknown_effects = !closed;
        if closed {
            if job.replace {
                job.outputs = job.inputs.clone();
                job.data.accesses = job.inputs.iter().map(FileAccess::read_write).collect();
            } else {
                job.outputs.push(shell.stdout.clone().unwrap());
                job.data.accesses = job.inputs.iter().map(FileAccess::read).collect();
            }
        } else {
            // An unknown option may consume an apparent operand or introduce
            // arbitrary files. Do not turn guessed operands into dependencies.
            job.inputs.clear();
        }
        Ok(job)
    }

    fn batchable(&self) -> bool {
        cfg!(unix)
            && !self.data.unknown_effects
            && self.batch_safe
            && (self.replace || !self.data.shell.ignore_failure)
            && self.data.shell.stderr.as_deref().is_none_or(is_null_device)
            && (!self.replace || self.data.shell.stdout.as_deref().is_none_or(is_null_device))
            && self.inputs.iter().chain(&self.outputs).all(|name| {
                !name.contains(['"', '\''])
                    && !name.chars().any(|ch| ch.is_whitespace() && ch != ' ')
                    && !name.contains("//")
                    && !name
                        .split(['/', '\\'])
                        .any(|part| matches!(part, "." | ".."))
            })
    }

    fn compatible(&self, other: &Self) -> bool {
        other.batchable()
            && self.args == other.args
            && self.replace == other.replace
            && self.data.shell.ignore_failure == other.data.shell.ignore_failure
            && self.data.shell.stderr == other.data.shell.stderr
            && (!self.replace || self.data.shell.stdout == other.data.shell.stdout)
    }

    fn conflicts(&self, other: &Self) -> bool {
        self.outputs
            .iter()
            .any(|path| other.inputs.contains(path) || other.outputs.contains(path))
            || other.outputs.iter().any(|path| self.inputs.contains(path))
    }

    fn execute_group(jobs: &[&Self]) -> Result<()> {
        let first = jobs[0];
        finish(
            &first.data.shell,
            (|| {
                let independent = match guard_paths(jobs) {
                    Ok(independent) => independent,
                    Err(_) if first.data.shell.ignore_failure => false,
                    Err(error) => return Err(error),
                };
                // Conflict comments/#error text embeds the original input name.
                // Keep original stdout boundaries for potentially defining source;
                // backslashes/trigraphs can hide a spliced directive keyword.
                let filename_sensitive = !first.replace
                    && jobs.iter().try_fold(false, |found, job| {
                        if found {
                            return Ok::<_, anyhow::Error>(true);
                        }
                        let mut source = BufReader::new(File::open(&job.inputs[0])?);
                        let mut line = Vec::new();
                        while source.read_until(b'\n', &mut line)? != 0 {
                            if line.contains(&b'\\')
                                || line.contains(&b'?')
                                || line.windows(6).any(|word| word == b"define")
                                || line.windows(5).any(|word| word == b"undef")
                            {
                                return Ok(true);
                            }
                            line.clear();
                        }
                        Ok(false)
                    })?;
                if !independent || filename_sensitive {
                    // Conflicting jobs must retain their individual boundaries.
                    for job in jobs {
                        job.execute()?;
                    }
                    return Ok(());
                }
                if !first.replace {
                    let copied = (|| -> Result<()> {
                        for job in jobs {
                            let mut input = File::open(&job.inputs[0])
                                .with_context(|| format!("opening coan input {}", job.inputs[0]))?;
                            let mut output = File::create(&job.outputs[0]).with_context(|| {
                                format!("opening coan output {}", job.outputs[0])
                            })?;
                            // Preserve shell-redirection permissions, unlike fs::copy.
                            io::copy(&mut input, &mut output)?;
                        }
                        Ok(())
                    })();
                    if copied.is_err() && first.data.shell.ignore_failure {
                        for job in jobs {
                            job.execute()?;
                        }
                        return Ok(());
                    }
                    copied?;
                }
                let (program, args) = first.args.split_first().context("coan has no executable")?;
                let mut command = Command::new(program);
                command.args(args);
                if !first.replace {
                    command.arg("--replace");
                }
                // Source-form stdout belongs to each copied output, not to the
                // shared child. Replacement mode itself emits no transformed stdout.
                let shell = ShellHints::new(
                    if first.replace {
                        first.data.shell.stdout.clone()
                    } else {
                        None
                    },
                    first.data.shell.stderr.clone(),
                    first.data.shell.ignore_failure,
                );
                configure_stdio(&mut command, &shell)?;
                command.stdin(Stdio::piped());
                let mut child = command
                    .spawn()
                    .with_context(|| format!("starting coan batch {program}"))?;
                let write_result = (|| -> Result<()> {
                    let mut input = child
                        .stdin
                        .take()
                        .context("opening coan filename-list pipe")?;
                    for job in jobs {
                        for name in &job.outputs {
                            // Coan quotes spaces in stdin filename lists, but has no
                            // portable escaping for embedded quotes or newlines.
                            writeln!(input, "\"{name}\"")?;
                        }
                    }
                    Ok(())
                })();
                let status = child.wait().context("waiting for coan batch")?;
                check_status(status)?;
                write_result.context("writing coan stdin filename list")
            })(),
        )
    }
}

impl Job for Coan {
    fn data(&self) -> &JobData {
        &self.data
    }
    fn data_mut(&mut self) -> &mut JobData {
        &mut self.data
    }

    fn execute(&self) -> Result<()> {
        finish(
            &self.data.shell,
            (|| {
                if !self.data.unknown_effects {
                    guard_paths(&[self])?;
                }
                check_status(command_status(&self.argv, &self.data.shell)?)
            })(),
        )
    }

    fn execute_batch(&self, items: &[&dyn Job]) -> Result<()> {
        let jobs = items
            .iter()
            .map(|item| {
                (*item as &dyn Any)
                    .downcast_ref::<Self>()
                    .context("non-coan job in coan batch")
            })
            .collect::<Result<Vec<_>>>()?;
        let mut index = 0;
        while index < jobs.len() {
            let first = jobs[index];
            let mut end = index + 1;
            if first.batchable() {
                while end < jobs.len()
                    && first.compatible(jobs[end])
                    && !jobs[index..end].iter().any(|job| job.conflicts(jobs[end]))
                {
                    end += 1;
                }
            }
            if end == index + 1 {
                first.execute()?;
            } else {
                Self::execute_group(&jobs[index..end])?;
            }
            index = end;
        }
        Ok(())
    }
}

/// Validate at execution time, so generated files need not exist while parsing.
/// Return false for cross-job aliases: they may run separately but cannot fuse.
fn guard_paths(jobs: &[&Coan]) -> Result<bool> {
    let mut seen: Vec<(&str, Option<Handle>, bool, usize)> = Vec::new();
    let mut independent = true;
    for (index, job) in jobs.iter().enumerate() {
        let mut paths: Vec<(&str, bool)> = if job.replace {
            job.inputs
                .iter()
                .map(|path| (path.as_str(), true))
                .collect()
        } else {
            job.inputs
                .iter()
                .map(|path| (path.as_str(), false))
                .chain(job.outputs.iter().map(|path| (path.as_str(), true)))
                .collect()
        };
        if let Some(stdout) = &job.data.shell.stdout {
            if !is_null_device(stdout) && (job.replace || !job.outputs.contains(stdout)) {
                paths.push((stdout, true));
            }
        }
        if let Some(stderr) = &job.data.shell.stderr {
            if !is_null_device(stderr) {
                paths.push((stderr, true));
            }
        }
        for (path, writes) in paths {
            reject_symlink_components([Path::new(path)])?;
            let metadata = if writes && !job.replace {
                destination_metadata(Path::new(path))?
            } else if job.inputs.iter().any(|input| input == path) {
                Some(checked_regular(Path::new(path))?)
            } else {
                destination_metadata(Path::new(path))?
            };
            let handle = if let Some(metadata) = metadata {
                if !metadata.is_file() {
                    bail!("coan requires a regular file: {path}");
                }
                let handle =
                    Handle::from_path(path).with_context(|| format!("opening coan file {path}"))?;
                #[cfg(unix)]
                if writes && !single_link(handle.as_file())? {
                    bail!("coan output may have hard-link aliases: {path}");
                }
                Some(handle)
            } else {
                None
            };
            for (other, identity, other_writes, owner) in &seen {
                if (writes || *other_writes)
                    && (path == *other
                        || handle
                            .as_ref()
                            .zip(identity.as_ref())
                            .is_some_and(|(a, b)| a == b))
                {
                    if index == *owner {
                        bail!("coan input/output paths alias: {path} and {other}");
                    }
                    independent = false;
                }
            }
            seen.push((path, handle, writes, index));
        }
    }
    Ok(independent)
}

#[cfg(unix)]
fn single_link(file: &File) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;
    Ok(file.metadata()?.nlink() == 1)
}

fn check_status(status: ExitStatus) -> Result<()> {
    match status.code() {
        Some(code) if (0..=255).contains(&code) && code & (4 | 8) == 0 => Ok(()),
        _ => bail!("coan failed ({status})"),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::check_status;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    #[test]
    fn status_bits_and_signals_are_distinct() {
        for code in 0..=255 {
            assert_eq!(
                check_status(ExitStatus::from_raw(code << 8)).is_ok(),
                code & (4 | 8) == 0,
                "exit code {code}"
            );
        }
        for signal in [9, 15, 17] {
            assert!(check_status(ExitStatus::from_raw(signal)).is_err());
        }
    }
}
