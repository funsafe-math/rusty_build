use std::fs;
use std::io;
use std::path::Path;

use anyhow::{Context, Result, bail};

pub(crate) fn checked_regular(path: &Path) -> Result<fs::Metadata> {
    let metadata =
        fs::symlink_metadata(path).with_context(|| format!("inspecting {}", path.display()))?;
    if !metadata.file_type().is_file() {
        bail!(
            "native file operation requires a regular non-symlink file: {}",
            path.display()
        );
    }
    Ok(metadata)
}

pub(crate) fn destination_metadata(path: &Path) -> Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(_) => checked_regular(path).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("inspecting {}", path.display())),
    }
}

/// Check the supplied spelling directly, including existing parent components.
/// Never resolve an alias into a different path for execution or graph metadata.
pub(crate) fn reject_symlink_components<'a>(
    paths: impl IntoIterator<Item = &'a Path>,
) -> Result<()> {
    for path in paths {
        for ancestor in path.ancestors() {
            match fs::symlink_metadata(ancestor) {
                Ok(metadata) if metadata.file_type().is_symlink() => bail!(
                    "native operation does not support symlink path components: {}",
                    ancestor.display()
                ),
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("inspecting {}", ancestor.display()));
                }
            }
        }
    }
    Ok(())
}
