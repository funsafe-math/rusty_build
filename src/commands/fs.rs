use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

pub(crate) fn absolute(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        cwd.join(path)
    }
}

pub(crate) fn checked_regular(path: &Path) -> Result<fs::Metadata> {
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

pub(crate) fn same_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
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

pub(crate) fn destination_metadata(path: &Path) -> Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(_) => checked_regular(path).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("inspecting {}", path.display())),
    }
}

pub(crate) fn singleton(metadata: &fs::Metadata) -> bool {
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

/// Recheck at execution time: an earlier opaque job may have created aliases
/// after graph construction. Native handlers must not trust stale paths.
pub(crate) fn reject_symlink_components<'a>(
    paths: impl IntoIterator<Item = &'a Path>,
    cwd: &Path,
) -> Result<()> {
    for path in paths {
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
    Ok(())
}
