//! File-effect inference is separate from external execution and cacheability.
use crate::{
    commands,
    job::{Job, JobSpec},
    shell::ShellCommand,
};
use anyhow::{Result, bail};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

mod inspection;
mod mutation;

#[derive(Debug)]
pub(crate) struct Effects {
    pub inputs: Vec<PathBuf>,
    pub outputs: Vec<PathBuf>,
    pub debug_lookup: bool,
}

pub(crate) fn parse(kind: &str, shell: &ShellCommand) -> JobSpec {
    let mut spec = commands::external_tool(kind, shell);
    let effects = match kind {
        "nm" | "objdump" | "readelf" | "size" | "strings" | "addr2line" | "c++filt" | "readobj" => {
            inspection::effects(kind, shell)
        }
        "objcopy" | "strip" | "ar" | "ranlib" | "as" | "ld" | "elfedit" | "dwp" => {
            mutation::effects(kind, shell)
        }
        _ => Err(format!(
            "{kind} may discover source/debug files; complete effects are not modeled"
        )),
    };
    match effects {
        Ok(effects) if effects.debug_lookup => {
            spec.inputs = effects.inputs;
            spec.outputs = effects.outputs;
            spec.parsed["effect_reason"] =
                "mode may discover separate debug files; complete inputs are not known".into();
        }
        Ok(effects) => {
            spec.inputs = effects.inputs;
            spec.outputs = effects.outputs;
            spec.unknown_effects = false;
            spec.parsed["argument_parsing"] = "effects".into();
            spec.parsed["effect_reason"] =
                "known file operands and supported option semantics".into();
        }
        Err(reason) => {
            spec.parsed["effect_reason"] = reason.into();
        }
    }
    spec
}

fn is_precise(job: &Job) -> bool {
    job.spec
        .parsed
        .get("argument_parsing")
        .and_then(|v| v.as_str())
        == Some("effects")
        && !job.spec.unknown_effects
}

/// Graph aliases and thin-archive members cannot be inferred from plain argv.
/// Existing unsupported paths fall back before scheduling; new ones are refused
/// before launching a precise job, rather than execute against stale hazards.
fn uncertainty(job: &Job, cwd: &Path) -> Result<Option<String>> {
    for path in job.spec.inputs.iter().chain(&job.spec.outputs) {
        let absolute = if path.is_absolute() {
            path.clone()
        } else {
            cwd.join(path)
        };
        for ancestor in absolute.ancestors() {
            if fs::symlink_metadata(ancestor).is_ok_and(|m| m.file_type().is_symlink()) {
                return Ok(Some(format!(
                    "symlink alias in file effects: {}",
                    ancestor.display()
                )));
            }
        }
        let meta = match fs::metadata(&absolute) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => continue, // The original tool reports access failures.
        };
        if !meta.is_file() {
            return Ok(Some(format!("non-regular operand: {}", path.display())));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if meta.nlink() > 1 {
                return Ok(Some(format!(
                    "hardlink alias in file effects: {}",
                    path.display()
                )));
            }
        }
    }
    if !matches!(job.spec.kind.as_str(), "strings" | "c++filt") {
        for path in &job.spec.inputs {
            let absolute = if path.is_absolute() {
                path.clone()
            } else {
                cwd.join(path)
            };
            if let Ok(mut file) = fs::File::open(&absolute) {
                let mut magic = [0u8; 8];
                match file.read_exact(&mut magic) {
                    Ok(()) if &magic == b"!<thin>\n" => {
                        return Ok(Some(format!(
                            "thin archive has external member inputs: {}",
                            path.display()
                        )));
                    }
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {}
                    Err(error) => {
                        return Ok(Some(format!(
                            "cannot validate input format {}: {error}",
                            path.display()
                        )));
                    }
                }
            }
        }
    }
    Ok(None)
}

pub(crate) fn refine_existing(job: &mut Job, cwd: &Path) -> Result<()> {
    if is_precise(job) {
        if let Some(reason) = uncertainty(job, cwd)? {
            job.spec.unknown_effects = true;
            job.spec.parsed["argument_parsing"] = "passthrough".into();
            job.spec.parsed["effect_reason"] = reason.into();
        }
    }
    Ok(())
}

pub(crate) fn validate_runtime(job: &Job, cwd: &Path) -> Result<()> {
    if is_precise(job) {
        if let Some(reason) = uncertainty(job, cwd)? {
            bail!(
                "precise tool effects no longer valid ({reason}); re-plan now that the operand exists"
            );
        }
    }
    Ok(())
}
