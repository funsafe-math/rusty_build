use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::Ordering;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::job::{
    BatchContext, BatchItem, BatchReport, ExecutionContext, Job, JobRecord, JobSpec,
    has_file_redirects, is_null_device,
};
use crate::registry::command_pattern;
use crate::shell::ShellCommand;

use super::fs::{absolute, checked_regular, reject_symlink_components, singleton};

/// Coan remains an external preprocessor. Only an explicitly closed replacement
/// invocation has modeled effects; all other modes preserve argv conservatively.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Coan {
    pub argv: Vec<String>,
    pub mode: Option<String>,
    pub files: Vec<PathBuf>,
    pub replace: bool,
    closed: bool,
    non_file_argv: Vec<String>,
}

impl Job for Coan {
    const TYPE: &'static str = "coan";

    fn patterns() -> Vec<String> {
        vec![command_pattern("coan")]
    }

    fn parse(shell: &ShellCommand) -> Result<Self> {
        let argv: Vec<_> = std::iter::once(shell.program.clone())
            .chain(shell.args.iter().cloned())
            .collect();
        let mut job = Self {
            mode: shell.args.first().cloned(),
            non_file_argv: vec![shell.program.clone()],
            argv,
            files: Vec::new(),
            replace: false,
            closed: false,
        };
        if !matches!(
            job.mode.as_deref(),
            Some(
                "source"
                    | "spin"
                    | "symbols"
                    | "includes"
                    | "defs"
                    | "pragmas"
                    | "lines"
                    | "directives"
                    | "errors"
            )
        ) {
            return Ok(job);
        }
        job.non_file_argv.push(shell.args[0].clone());
        let mut index = 1;
        let mut operands_only = false;
        let mut opaque_configuration = false;
        while index < shell.args.len() {
            let arg = &shell.args[index];
            if operands_only || !arg.starts_with('-') || arg == "-" {
                job.files.push(PathBuf::from(arg));
                index += 1;
                continue;
            }
            job.non_file_argv.push(arg.clone());
            if arg == "--" {
                operands_only = true;
            } else if matches!(arg.as_str(), "-r" | "--replace") {
                job.replace = true;
            } else if matches!(arg.as_str(), "-R" | "--recurse") {
                opaque_configuration = true;
            } else if matches!(
                arg.as_str(),
                "-m" | "--implicit"
                    | "-V"
                    | "--verbose"
                    | "-K"
                    | "--keepgoing"
                    | "--no-transients"
                    | "-P"
                    | "--pod"
                    | "-E"
                    | "--eval-wip"
                    | "--line"
                    | "-c"
                    | "--complement"
            ) {
                // These options have no additional file inputs or outputs.
            } else {
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
                );
                let short_value = arg.as_bytes().get(1).is_some_and(|flag| {
                    matches!(flag, b'D' | b'U' | b'g' | b'k' | b'x' | b'F' | b'b' | b'f')
                }) && !arg.starts_with("--");
                if matches!(long, "--backup" | "--file")
                    || (short_value && matches!(arg.as_bytes()[1], b'b' | b'f'))
                {
                    opaque_configuration = true;
                }
                if !long_value && !short_value {
                    // In particular, do not infer backups, recursion, argument
                    // files, or the arity of a future option.
                    job.files.clear();
                    return Ok(job);
                }
                if (long_value && !arg.contains('=')) || (short_value && arg.len() == 2) {
                    index += 1;
                    let value = shell
                        .args
                        .get(index)
                        .with_context(|| format!("coan option {arg} requires a value"))?;
                    if value.is_empty() {
                        bail!("coan option {arg} requires a nonempty value");
                    }
                    job.non_file_argv.push(value.clone());
                } else if long_value && arg.ends_with('=') {
                    bail!("coan option {long} requires a nonempty value");
                }
            }
            index += 1;
        }
        job.closed = job.mode.as_deref() == Some("source")
            && job.replace
            && !opaque_configuration
            && !job.files.is_empty()
            && !job.files.iter().any(|path| path == Path::new("-"))
            && safe_redirections(shell);
        Ok(job)
    }

    fn inputs(&self) -> impl Iterator<Item = &Path> {
        self.files
            .iter()
            .filter(|_| self.closed)
            .map(PathBuf::as_path)
    }

    fn outputs(&self) -> impl Iterator<Item = &Path> {
        self.files
            .iter()
            .filter(|_| self.closed)
            .map(PathBuf::as_path)
    }

    fn unknown_effects(&self) -> bool {
        !self.closed
    }

    fn refine_effects(&self, spec: &mut JobSpec, cwd: &Path) -> Result<()> {
        if self.closed && (!cfg!(unix) || self.check_targets(cwd, true).is_err()) {
            // Preserve intrinsic paths for snapshot restoration. Unknown effects
            // are an additional conservative overlay, never removed metadata.
            // Link-count guards are unavailable outside Unix, so those hosts
            // execute exact argv conservatively rather than rejecting the job.
            spec.unknown_effects = true;
            spec.effect_reason =
                Some("coan replacement targets cannot be proven closed regular files".into());
        }
        Ok(())
    }

    fn validate(&self, record: &JobRecord, cwd: &Path) -> Result<()> {
        if self.closed && !record.spec.unknown_effects {
            self.check_targets(cwd, false)?;
        }
        Ok(())
    }

    fn execute(&self, ctx: &ExecutionContext<'_>) -> Result<()> {
        self.validate(ctx.record, ctx.cwd)?;
        let (program, args) = self.argv.split_first().context("coan has no executable")?;
        let mut command = Command::new(program);
        command.args(args).current_dir(ctx.cwd);
        // The recognized `|| cd .` suffix is intentionally not executed:
        // coan's own status bits, including errors, are authoritative here.
        if let Some(shell) = &ctx.record.shell {
            if let Some(path) = &shell.stdin {
                command.stdin(if is_null_device(path) {
                    Stdio::null()
                } else {
                    Stdio::from(
                        File::open(ctx.absolute(path))
                            .with_context(|| format!("opening coan stdin {}", path.display()))?,
                    )
                });
            }
            if let Some(path) = &shell.stdout {
                command.stdout(output_stdio(path, ctx.cwd, shell.append_stdout)?);
            }
            if let Some(path) = &shell.stderr {
                command.stderr(output_stdio(path, ctx.cwd, false)?);
            }
        }
        let status = command
            .status()
            .with_context(|| format!("starting coan {program}"))?;
        check_status(status)
            .with_context(|| format!("coan command on line {}", ctx.record.source.line))
    }

    fn execute_batch(items: &[BatchItem<'_, Self>], ctx: &BatchContext<'_>) -> BatchReport {
        let mut report = BatchReport::default();
        let mut index = 0;
        while index < items.len() && !ctx.cancelled.load(Ordering::Acquire) {
            if !items[index].job.batchable(items[index].record) {
                let result = ctx.run_each(&items[index..index + 1], |job, execution| {
                    job.execute(execution)
                });
                report.outcomes.extend(result.outcomes);
                index += 1;
                continue;
            }
            // Only reorder the closed, independent part of this scheduler wave;
            // uncertain commands retain their original execution boundaries.
            let mut groups = BTreeMap::<(&[String], bool, bool), Vec<&BatchItem<'_, Self>>>::new();
            while index < items.len() && items[index].job.batchable(items[index].record) {
                let item = &items[index];
                let (quiet_stdout, quiet_stderr) = quiet_outputs(item.record);
                groups
                    .entry((&item.job.non_file_argv, quiet_stdout, quiet_stderr))
                    .or_default()
                    .push(item);
                index += 1;
            }
            for group in groups.values() {
                if ctx.cancelled.load(Ordering::Acquire) {
                    break;
                }
                let result = execute_group(group, ctx);
                if result.is_err() {
                    ctx.cancelled.store(true, Ordering::Release);
                }
                match result {
                    Ok(()) => report
                        .outcomes
                        .extend(group.iter().map(|item| (item.id, Ok(())))),
                    Err(error) => {
                        // Aggregate errors cannot identify which earlier files
                        // were edited successfully. Never certify any of them.
                        let message = format!("{error:#}");
                        report.outcomes.extend(
                            group
                                .iter()
                                .map(|item| (item.id, Err(anyhow!(message.clone())))),
                        );
                    }
                }
            }
        }
        report
    }
}

impl Coan {
    fn check_targets(&self, cwd: &Path, allow_missing: bool) -> Result<()> {
        reject_symlink_components(self.files.iter().map(PathBuf::as_path), cwd)?;
        let mut targets = BTreeSet::new();
        for path in &self.files {
            let path = absolute(cwd, path);
            let metadata = match checked_regular(&path) {
                Ok(metadata) => metadata,
                Err(error)
                    if allow_missing
                        && error
                            .downcast_ref::<io::Error>()
                            .is_some_and(|error| error.kind() == io::ErrorKind::NotFound) =>
                {
                    continue;
                }
                Err(error) => return Err(error),
            };
            if !singleton(&metadata) {
                bail!(
                    "coan replacement target may have hard-link aliases: {}",
                    path.display()
                );
            }
            if !targets.insert(path.canonicalize()?) {
                bail!("coan replacement repeats a target: {}", path.display());
            }
        }
        Ok(())
    }

    fn batchable(&self, record: &JobRecord) -> bool {
        self.closed
            && !record.spec.unknown_effects
            && record.shell.as_ref().is_some_and(safe_redirections)
            && self.files.iter().all(|path| {
                path.to_str()
                    .is_some_and(|name| !name.contains(['"', '\n', '\r']))
            })
    }
}

fn safe_redirections(shell: &ShellCommand) -> bool {
    shell.stdin.is_none() && !has_file_redirects(Some(shell))
}

fn quiet_outputs(record: &JobRecord) -> (bool, bool) {
    record.shell.as_ref().map_or((false, false), |shell| {
        (
            shell.stdout.as_deref().is_some_and(is_null_device),
            shell.stderr.as_deref().is_some_and(is_null_device),
        )
    })
}

fn output_stdio(path: &Path, cwd: &Path, append: bool) -> Result<Stdio> {
    if is_null_device(path) {
        Ok(Stdio::null())
    } else {
        Ok(Stdio::from(output_file(&absolute(cwd, path), append)?))
    }
}

fn output_file(path: &Path, append: bool) -> Result<File> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .append(append)
        .truncate(!append)
        .open(path)
        .with_context(|| format!("opening coan output {}", path.display()))
}

fn check_status(status: ExitStatus) -> Result<()> {
    match status.code() {
        Some(code) if (0..=255).contains(&code) && code & (4 | 8) == 0 => Ok(()),
        _ => bail!("coan failed ({status})"),
    }
}

fn execute_group(items: &[&BatchItem<'_, Coan>], ctx: &BatchContext<'_>) -> Result<()> {
    let mut targets = BTreeSet::new();
    for item in items {
        item.job.validate(item.record, ctx.cwd)?;
        for path in &item.job.files {
            if !targets.insert(absolute(ctx.cwd, path).canonicalize()?) {
                bail!(
                    "coan batch contains aliased replacement targets: {}",
                    path.display()
                );
            }
        }
    }
    if ctx.cancelled.load(Ordering::Acquire) {
        bail!("coan batch cancelled before execution");
    }
    let (program, args) = items[0]
        .job
        .non_file_argv
        .split_first()
        .context("coan has no executable")?;
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(ctx.cwd)
        .stdin(Stdio::piped());
    let (quiet_stdout, quiet_stderr) = quiet_outputs(items[0].record);
    if quiet_stdout {
        command.stdout(Stdio::null());
    }
    if quiet_stderr {
        command.stderr(Stdio::null());
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("starting coan batch {program}"))?;
    let write_result = (|| -> Result<()> {
        let mut input = child
            .stdin
            .take()
            .context("opening coan filename-list pipe")?;
        for item in items {
            for path in &item.job.files {
                // Coan's stdin filename format quotes spaces, but does not
                // provide a portable escape for embedded quotes or newlines.
                writeln!(
                    input,
                    "\"{}\"",
                    path.to_str().context("non-UTF-8 coan filename")?
                )?;
            }
        }
        Ok(())
    })();
    let status = child.wait().context("waiting for coan batch")?;
    check_status(status)?;
    write_result.context("writing coan stdin filename list")
}

#[cfg(all(test, unix))]
mod tests {
    use super::check_status;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    #[test]
    fn status_bits_are_not_generic_nonzero_failures_or_signal_codes() {
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
