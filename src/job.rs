use std::fs::{self, File, FileTimes, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::input::SourceCommand;
use crate::shell::ShellCommand;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExecutionStrategy {
    External,
    Batched,
    Internal,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Operation {
    Copy {
        src: PathBuf,
        dst: PathBuf,
    },
    CopyChain {
        src: PathBuf,
        mid: PathBuf,
        dst: PathBuf,
    },
    Mkdir {
        paths: Vec<PathBuf>,
        parents: bool,
    },
    Touch {
        paths: Vec<PathBuf>,
    },
    Remove {
        paths: Vec<PathBuf>,
        force: bool,
    },
    Move {
        src: PathBuf,
        dst: PathBuf,
    },
    External,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobSpec {
    pub kind: String,
    pub operation: Operation,
    pub inputs: Vec<PathBuf>,
    pub outputs: Vec<PathBuf>,
    pub unknown_effects: bool,
    pub cacheable: bool,
    pub parsed: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub source: SourceCommand,
    pub shell: Option<ShellCommand>,
    pub spec: JobSpec,
    pub strategy: ExecutionStrategy,
}

fn absolute(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        cwd.join(path)
    }
}

/// External jobs always execute the original text, never reconstructed argv.
pub fn execute(job: &Job, cwd: &Path) -> Result<()> {
    crate::binutils::validate_runtime(job, cwd)?;
    // Native handlers cannot silently discard redirection. Classification normally
    // turns these into barriers, but retain this guard for public/custom jobs.
    let redirected = job
        .shell
        .as_ref()
        .is_some_and(|s| s.stdin.is_some() || s.stdout.is_some() || s.stderr.is_some());
    if job.strategy == ExecutionStrategy::External || redirected {
        return external(job, cwd);
    }
    if matches!(
        job.spec.operation,
        Operation::Copy { .. }
            | Operation::CopyChain { .. }
            | Operation::Mkdir { .. }
            | Operation::Touch { .. }
    ) {
        // Opaque earlier jobs may create aliases after graph construction.
        // Refuse symlink components at runtime rather than trust stale paths.
        for path in job.spec.inputs.iter().chain(&job.spec.outputs) {
            let path = absolute(cwd, path);
            for ancestor in path.ancestors() {
                if fs::symlink_metadata(ancestor).is_ok_and(|m| m.file_type().is_symlink()) {
                    bail!(
                        "native operation does not support symlink path components: {}",
                        ancestor.display()
                    );
                }
            }
        }
    }
    match &job.spec.operation {
        Operation::Copy { src, dst } => copy_file(&absolute(cwd, src), &absolute(cwd, dst)),
        Operation::CopyChain { src, mid, dst } => copy_chain(
            &absolute(cwd, src),
            &absolute(cwd, mid),
            &absolute(cwd, dst),
        ),
        Operation::Mkdir { paths, parents } => {
            for path in paths {
                let path = absolute(cwd, path);
                if *parents {
                    mkdir_parents(&path)?;
                } else {
                    fs::create_dir(&path).with_context(|| format!("mkdir {}", path.display()))?;
                }
            }
            Ok(())
        }
        Operation::Touch { paths } => {
            for path in paths {
                touch(&absolute(cwd, path))?;
            }
            Ok(())
        }
        Operation::Remove { paths, force } => {
            for path in paths {
                let path = absolute(cwd, path);
                match fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(error) if *force && error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(error).with_context(|| format!("rm {}", path.display()));
                    }
                }
            }
            Ok(())
        }
        Operation::Move { src, dst } => move_file(&absolute(cwd, src), &absolute(cwd, dst)),
        Operation::External => external(job, cwd),
    }
}

fn external(job: &Job, cwd: &Path) -> Result<()> {
    let status = Command::new("bash")
        .args(["--noprofile", "--norc", "-c"])
        .arg(&job.source.text)
        .env_remove("BASH_ENV")
        .env_remove("ENV")
        .current_dir(cwd)
        .status()
        .context("starting bash for original command")?;
    if !status.success() {
        bail!(
            "command on line {} failed ({status}): {}",
            job.source.line,
            job.source.text
        );
    }
    Ok(())
}

fn checked_regular(path: &Path) -> Result<fs::Metadata> {
    let metadata =
        fs::symlink_metadata(path).with_context(|| format!("inspecting {}", path.display()))?;
    if !metadata.file_type().is_file() {
        bail!(
            "native file operation requires a regular non-symlink file: {}",
            path.display()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o7000 != 0 {
            bail!(
                "native file operation does not support special permission bits: {}",
                path.display()
            );
        }
    }
    Ok(metadata)
}

fn same_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        a.dev() == b.dev() && a.ino() == b.ino()
    }
    #[cfg(not(unix))]
    {
        let _ = (a, b);
        false
    }
}

fn destination_metadata(path: &Path) -> Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(_) => checked_regular(path).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("inspecting {}", path.display())),
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

fn singleton(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.nlink() == 1
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
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

fn mkdir_parents(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    let components: Vec<_> = path.components().collect();
    for (index, component) in components.iter().enumerate() {
        current.push(component.as_os_str());
        match fs::create_dir(&current) {
            Ok(()) => {
                // GNU mkdir -p gives newly created intermediate directories
                // owner write/search even when umask removes those bits.
                #[cfg(unix)]
                if index + 1 < components.len() {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = fs::metadata(&current)?.permissions().mode();
                    fs::set_permissions(&current, fs::Permissions::from_mode(mode | 0o300))?;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && current.is_dir() => {}
            Err(error) => {
                return Err(error).with_context(|| format!("mkdir -p {}", current.display()));
            }
        }
    }
    if components.is_empty() {
        bail!("mkdir: empty path");
    }
    Ok(())
}

fn touch(path: &Path) -> Result<()> {
    let file = match fs::symlink_metadata(path) {
        Ok(_) => {
            if !singleton(&checked_regular(path)?) {
                bail!(
                    "native touch does not support multiply-linked files: {}",
                    path.display()
                );
            }
            File::open(path).with_context(|| format!("opening {} for touch", path.display()))?
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            OpenOptions::new().write(true).create_new(true).open(path)?
        }
        Err(error) => return Err(error.into()),
    };
    let now = std::time::SystemTime::now();
    file.set_times(FileTimes::new().set_accessed(now).set_modified(now))?;
    Ok(())
}

fn move_file(src: &Path, dst: &Path) -> Result<()> {
    let source = checked_regular(src)?;
    if let Some(destination) = destination_metadata(dst)? {
        if same_file(&source, &destination) {
            bail!("mv source and destination are the same file");
        }
    }
    // Cross-device moves require metadata and cleanup semantics beyond this
    // narrow subset. rename reports EXDEV before mutating either location.
    fs::rename(src, dst).with_context(|| {
        format!(
            "mv {} to {} (cross-device moves unsupported)",
            src.display(),
            dst.display()
        )
    })?;
    Ok(())
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
    fn mkdir_parent_subset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a/b");
        mkdir_parents(&path).unwrap();
        mkdir_parents(&path).unwrap();
        assert!(path.is_dir());
        assert!(copy_file(dir.path(), &dir.path().join("file")).is_err());
    }

    #[test]
    fn native_touch_move_and_force_remove() {
        let dir = tempfile::tempdir().unwrap();
        let run = |operation| {
            execute(
                &Job {
                    source: SourceCommand {
                        line: 1,
                        text: String::new(),
                        target: None,
                    },
                    shell: None,
                    spec: JobSpec {
                        kind: "test".into(),
                        operation,
                        inputs: vec![],
                        outputs: vec![],
                        unknown_effects: true,
                        cacheable: false,
                        parsed: serde_json::json!({}),
                    },
                    strategy: ExecutionStrategy::Internal,
                },
                dir.path(),
            )
        };
        run(Operation::Touch {
            paths: vec!["a".into()],
        })
        .unwrap();
        assert!(dir.path().join("a").is_file());
        fs::write(dir.path().join("a"), "payload").unwrap();
        run(Operation::Move {
            src: "a".into(),
            dst: "b".into(),
        })
        .unwrap();
        assert!(!dir.path().join("a").exists());
        assert_eq!(fs::read(dir.path().join("b")).unwrap(), b"payload");
        run(Operation::Remove {
            paths: vec!["b".into()],
            force: false,
        })
        .unwrap();
        run(Operation::Remove {
            paths: vec!["b".into()],
            force: true,
        })
        .unwrap();
        assert!(
            run(Operation::Remove {
                paths: vec!["b".into()],
                force: false
            })
            .is_err()
        );
    }
}
