use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Parser;
use serde::{Deserialize, Serialize};

use crate::job::{
    BatchContext, BatchItem, BatchReport, ExecutionContext, ExecutionStrategy, Job, JobRecord,
};
use crate::registry::command_pattern;
use crate::shell::ShellCommand;

use super::fs::{
    checked_regular, destination_metadata, reject_symlink_components, same_file, singleton,
};

/// The supported GNU cp subset: exactly two ordinary file operands, no flags.
#[derive(Clone, Debug, Parser, Serialize, Deserialize)]
#[command(disable_help_flag = true, disable_version_flag = true)]
pub struct Cp {
    pub src: PathBuf,
    pub dst: PathBuf,
}

impl Job for Cp {
    const TYPE: &'static str = "cp";

    fn patterns() -> Vec<String> {
        vec![command_pattern("cp")]
    }

    fn parse(shell: &ShellCommand) -> Result<Self> {
        Ok(Self::try_parse_from(
            std::iter::once(shell.program.as_str()).chain(shell.args.iter().map(String::as_str)),
        )?)
    }

    fn inputs(&self) -> impl Iterator<Item = &Path> {
        std::iter::once(self.src.as_path())
    }

    fn outputs(&self) -> impl Iterator<Item = &Path> {
        std::iter::once(self.dst.as_path())
    }

    fn strategy(&self) -> ExecutionStrategy {
        ExecutionStrategy::Internal
    }

    fn cacheable(&self) -> bool {
        true
    }

    fn validate(&self, _record: &JobRecord, cwd: &Path) -> Result<()> {
        reject_symlink_components(self.inputs().chain(self.outputs()), cwd)
    }

    fn execute(&self, ctx: &ExecutionContext<'_>) -> Result<()> {
        copy_file(&ctx.absolute(&self.src), &ctx.absolute(&self.dst))
    }

    fn execute_batch(items: &[BatchItem<'_, Self>], ctx: &BatchContext<'_>) -> BatchReport {
        ctx.run_each(items, |job, execution| job.execute(execution))
    }
}

/// Internal copy fusion preserves both observable outputs and first-copy failure
/// ordering. It streams only when fresh, distinct destinations make that safe.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CopyChain {
    pub src: PathBuf,
    pub mid: PathBuf,
    pub dst: PathBuf,
}

impl CopyChain {
    pub fn new(src: PathBuf, mid: PathBuf, dst: PathBuf) -> Self {
        Self { src, mid, dst }
    }
}

impl Job for CopyChain {
    const TYPE: &'static str = "copy-chain";

    fn parse(_shell: &ShellCommand) -> Result<Self> {
        bail!("copy-chain is an internal rewrite and cannot be parsed from a command")
    }

    fn inputs(&self) -> impl Iterator<Item = &Path> {
        std::iter::once(self.src.as_path())
    }

    fn outputs(&self) -> impl Iterator<Item = &Path> {
        [self.mid.as_path(), self.dst.as_path()].into_iter()
    }

    fn strategy(&self) -> ExecutionStrategy {
        ExecutionStrategy::Internal
    }

    fn cacheable(&self) -> bool {
        true
    }

    fn validate(&self, _record: &JobRecord, cwd: &Path) -> Result<()> {
        reject_symlink_components(self.inputs().chain(self.outputs()), cwd)
    }

    fn execute(&self, ctx: &ExecutionContext<'_>) -> Result<()> {
        copy_chain(
            &ctx.absolute(&self.src),
            &ctx.absolute(&self.mid),
            &ctx.absolute(&self.dst),
        )
    }

    fn execute_batch(items: &[BatchItem<'_, Self>], ctx: &BatchContext<'_>) -> BatchReport {
        ctx.run_each(items, |job, execution| job.execute(execution))
    }
}

/// GNU cp's ordinary file subset: existing destination mode is retained, while
/// a new file uses source permissions filtered by the process umask.
fn copy_file(src: &Path, dst: &Path) -> Result<()> {
    let source_meta = checked_regular(src)?;
    let destination_meta = destination_metadata(dst)?;
    if destination_meta.as_ref().is_some_and(|m| !singleton(m)) {
        bail!(
            "native cp does not overwrite multiply-linked destinations: {}",
            dst.display()
        );
    }
    if destination_meta
        .as_ref()
        .is_some_and(|m| same_file(&source_meta, m))
    {
        bail!(
            "cp source and destination are the same file: {} and {}",
            src.display(),
            dst.display()
        );
    }
    let mut source = File::open(src).with_context(|| format!("opening {}", src.display()))?;
    let mut options = OpenOptions::new();
    options.write(true);
    if destination_meta.is_some() {
        options.create(false);
    } else {
        options.create_new(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options.mode(source_meta.permissions().mode() & 0o777);
    }
    let mut destination = options
        .open(dst)
        .with_context(|| format!("opening {}", dst.display()))?;
    // Check the opened descriptors again before any truncation.
    if same_file(&source.metadata()?, &destination.metadata()?) {
        bail!("cp source and destination refer to the same file");
    }
    destination.set_len(0)?;
    io::copy(&mut source, &mut destination)
        .with_context(|| format!("copying {} to {}", src.display(), dst.display()))?;
    Ok(())
}

fn missing(path: &Path) -> bool {
    matches!(fs::symlink_metadata(path), Err(e) if e.kind() == io::ErrorKind::NotFound)
}

fn copy_chain(src: &Path, mid: &Path, dst: &Path) -> Result<()> {
    let streamable = checked_regular(src).is_ok_and(|m| singleton(&m))
        && missing(mid)
        && missing(dst)
        && src != mid
        && src != dst
        && mid != dst
        && distinct_locations(mid, dst);
    if !streamable {
        // Preserve the original sequence and its ordering/failure behavior.
        copy_file(src, mid)?;
        return copy_file(mid, dst);
    }
    let metadata = checked_regular(src)?;
    let mut source = File::open(src)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options.mode(metadata.permissions().mode() & 0o777);
    }
    let mut intermediate = options
        .open(mid)
        .with_context(|| format!("creating {}", mid.display()))?;
    if let Err(error) = File::open(mid) {
        io::copy(&mut source, &mut intermediate)?;
        return Err(error).with_context(|| format!("opening {} for second copy", mid.display()));
    }
    // If creation of the second destination fails, still finish the original
    // first copy before returning its second-copy error.
    let mut final_file = match options.open(dst) {
        Ok(file) => file,
        Err(error) => {
            io::copy(&mut source, &mut intermediate)?;
            return Err(error).with_context(|| format!("creating {}", dst.display()));
        }
    };
    let mut buffer = [0u8; 64 * 1024];
    let mut second_error = None;
    loop {
        let length = match source.read(&mut buffer) {
            Ok(length) => length,
            Err(error) => {
                drop(final_file);
                fs::remove_file(dst).context("removing uncommitted second copy")?;
                return Err(error).context("reading first copy");
            }
        };
        if length == 0 {
            break;
        }
        if let Err(error) = intermediate.write_all(&buffer[..length]) {
            drop(final_file);
            fs::remove_file(dst).context("removing uncommitted second copy")?;
            return Err(error).context("writing first copy");
        }
        if second_error.is_none() {
            if let Err(error) = final_file.write_all(&buffer[..length]) {
                second_error = Some(error);
            }
        }
    }
    if let Some(error) = second_error {
        return Err(error).context("writing second copy");
    }
    Ok(())
}

fn distinct_locations(a: &Path, b: &Path) -> bool {
    match (
        a.parent().and_then(|p| p.canonicalize().ok()),
        b.parent().and_then(|p| p.canonicalize().ok()),
    ) {
        (Some(ap), Some(bp)) => {
            ap.join(a.file_name().unwrap_or_default()) != bp.join(b.file_name().unwrap_or_default())
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_overwrites_without_changing_destination_mode() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        fs::write(&src, "new").unwrap();
        fs::write(&dst, "old contents").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dst, fs::Permissions::from_mode(0o600)).unwrap();
        }
        copy_file(&src, &dst).unwrap();
        assert_eq!(fs::read(&dst).unwrap(), b"new");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(dst).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn copy_rejects_aliases_and_symlinks_without_truncation() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let alias = dir.path().join("alias");
        let link = dir.path().join("link");
        fs::write(&src, "original").unwrap();
        fs::hard_link(&src, &alias).unwrap();
        symlink(&src, &link).unwrap();
        assert!(copy_file(&src, &alias).is_err());
        assert!(copy_file(&src, &link).is_err());
        assert_eq!(fs::read(src).unwrap(), b"original");
    }

    #[test]
    fn copy_rejects_directory_source() {
        let dir = tempfile::tempdir().unwrap();
        assert!(copy_file(dir.path(), &dir.path().join("file")).is_err());
        assert!(!dir.path().join("file").exists());
    }

    #[test]
    fn chain_keeps_both_outputs_and_falls_back_for_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let mid = dir.path().join("mid");
        let dst = dir.path().join("dst");
        fs::write(&src, vec![42; 200_000]).unwrap();
        copy_chain(&src, &mid, &dst).unwrap();
        assert_eq!(fs::read(&mid).unwrap(), fs::read(&dst).unwrap());
        fs::write(&src, "replacement").unwrap();
        copy_chain(&src, &mid, &dst).unwrap();
        assert_eq!(fs::read(&mid).unwrap(), b"replacement");
        assert_eq!(fs::read(&dst).unwrap(), b"replacement");
    }

    #[test]
    fn chain_finishes_first_copy_when_second_destination_fails() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let mid = dir.path().join("mid");
        let dst = dir.path().join("missing/dst");
        fs::write(&src, vec![42; 200_000]).unwrap();
        assert!(copy_chain(&src, &mid, &dst).is_err());
        assert_eq!(fs::read(&mid).unwrap(), fs::read(&src).unwrap());
        assert!(!dst.exists());
    }
}
