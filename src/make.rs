//! GNU Make planning. A dry run is not a sandbox: Make expansion can execute code.

use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::input::{InputLog, read_log};

const SIDE_EFFECT_WARNING: &str = "GNU make planning is not side-effect-free: $(shell ...), $(file ...), included-makefile updates, and recursive or '+' recipes may execute during -n. Use only trusted makefiles; a failed planner may already have changed files. Captured recipes are replayed only after successful planning.";

/// Capture an ordered, traced, serial, unconditional GNU Make dry run.
///
/// Forward only explicitly supported options, goals, and ordinary variable
/// assignments. Inherited Make option variables are removed in the child only;
/// this function never changes the parent process's directory or environment.
/// The complete log is parsed only after the planner exits successfully.
pub fn plan(args: &[String], cwd: &Path) -> Result<InputLog> {
    validate_args(args)?;
    eprintln!("warning: {SIDE_EFFECT_WARNING}");
    let mut child = Command::new("make")
        .args(["-nB", "--trace", "-j1"])
        .args(args)
        .current_dir(cwd)
        .env_remove("MAKEFLAGS")
        .env_remove("MFLAGS")
        .env_remove("GNUMAKEFLAGS")
        .env_remove("MAKEOVERRIDES")
        .env_remove("MAKELEVEL")
        .env_remove("MAKE_RESTARTS")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("starting GNU make in {}", cwd.display()))?;

    let stdout = child
        .stdout
        .take()
        .context("GNU make stdout was not captured")?;
    let capture = (|| -> Result<String> {
        let mut reader = BufReader::new(stdout);
        let output = io::stdout();
        let mut output = output.lock();
        let mut line = Vec::new();
        let mut captured = Vec::new();
        loop {
            line.clear();
            if reader
                .read_until(b'\n', &mut line)
                .context("reading GNU make stdout")?
                == 0
            {
                break;
            }
            output
                .write_all(&line)
                .context("streaming GNU make stdout")?;
            output.flush().context("flushing GNU make stdout")?;
            captured.extend_from_slice(&line);
        }
        String::from_utf8(captured).context("GNU make output is not UTF-8")
    })();
    if capture.is_err() {
        // A reader/output failure must not leave an unwaited planner running.
        let _ = child.kill();
    }
    let status = child.wait().context("waiting for GNU make planner")?;
    let captured = capture?;
    if !status.success() {
        bail!(
            "GNU make planner failed ({status}); no captured recipes will be executed; planning itself may already have had side effects"
        );
    }
    let mut log = read_log(&captured).context("parsing GNU make plan")?;
    log.diagnostics.insert(
        0,
        crate::input::SourceDiagnostic {
            line: None,
            text: SIDE_EFFECT_WARNING.to_owned(),
        },
    );
    Ok(log)
}

fn validate_assignment(arg: &str) -> Result<()> {
    let Some((name, _)) = arg.split_once('=') else {
        return Ok(());
    };
    let name = name.trim_end_matches([':', '+', '?']);
    if name.is_empty()
        || !name
            .bytes()
            .enumerate()
            .all(|(i, c)| c == b'_' || c.is_ascii_alphabetic() || (i != 0 && c.is_ascii_digit()))
    {
        bail!("unsupported GNU make variable assignment: {arg}");
    }
    if matches!(
        name,
        "MAKE"
            | "MAKEFLAGS"
            | "MFLAGS"
            | "GNUMAKEFLAGS"
            | "MAKEOVERRIDES"
            | "MAKELEVEL"
            | "MAKE_RESTARTS"
    ) {
        bail!("GNU make planning control variable cannot be overridden: {name}");
    }
    Ok(())
}

fn validate_args(args: &[String]) -> Result<()> {
    let mut index = 0;
    let mut goals_only = false;
    while index < args.len() {
        let arg = &args[index];
        if goals_only || !arg.starts_with('-') || arg == "-" {
            validate_assignment(arg)?;
        } else if arg == "--" {
            goals_only = true;
        } else if let Some(long) = arg.strip_prefix("--") {
            let (name, value) = long
                .split_once('=')
                .map_or((long, None), |(n, v)| (n, Some(v)));
            match name {
                "file" | "makefile" | "include-dir" => {
                    if value.is_none() {
                        index += 1;
                        if index == args.len() {
                            bail!("missing argument for GNU make option {arg}");
                        }
                    }
                }
                "just-print"
                | "dry-run"
                | "recon"
                | "always-make"
                | "trace"
                | "no-builtin-rules"
                | "no-builtin-variables"
                | "warn-undefined-variables"
                    if value.is_none() => {}
                _ => bail!(
                    "unsupported GNU make planning option {arg}; cwd, jobs, execution mode, and output controls cannot be overridden"
                ),
            }
        } else {
            let mut flags = arg[1..].char_indices();
            while let Some((offset, flag)) = flags.next() {
                match flag {
                    'n' | 'B' | 'r' | 'R' => {}
                    'f' | 'I' => {
                        if offset + flag.len_utf8() == arg.len() - 1 {
                            index += 1;
                            if index == args.len() {
                                bail!("missing argument for GNU make option -{flag}");
                            }
                        }
                        break; // Remaining bytes, if any, are this option's operand.
                    }
                    _ => bail!(
                        "unsupported GNU make planning option -{flag} in {arg}; cwd, jobs, execution mode, and output controls cannot be overridden"
                    ),
                }
            }
        }
        index += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn fixture(makefile: &str) -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Makefile"), makefile).unwrap();
        dir
    }

    #[test]
    fn gnu_make_preserves_order_and_trace_targets_without_running_recipes() {
        let dir = fixture(
            ".PHONY: all first second\nall: first second\nfirst:\n\t@touch first-result\nsecond:\n\t@touch second-result\n",
        );
        let log = plan(&["all".into()], dir.path()).unwrap();
        assert_eq!(log.commands.len(), 2);
        assert_eq!(log.commands[0].text, "touch first-result");
        assert_eq!(log.commands[0].target.as_deref(), Some("first"));
        assert_eq!(log.commands[1].text, "touch second-result");
        assert_eq!(log.commands[1].target.as_deref(), Some("second"));
        assert!(!dir.path().join("first-result").exists());
        assert!(!dir.path().join("second-result").exists());
    }

    #[test]
    fn expansion_time_file_function_has_documented_side_effect() {
        let dir =
            fixture("$(file >expanded.txt,planning side effect)\nall:\n\t@touch recipe-result\n");
        let log = plan(&[], dir.path()).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("expanded.txt")).unwrap(),
            "planning side effect\n"
        );
        assert!(!dir.path().join("recipe-result").exists());
        assert!(
            log.diagnostics
                .iter()
                .any(|message| message.text.contains("not side-effect-free"))
        );
    }

    #[test]
    fn failed_planner_never_returns_a_partial_log() {
        let dir = fixture(
            ".PHONY: all first\nall: first missing-prerequisite\nfirst:\n\t@touch never-replay\n",
        );
        let error = plan(&[], dir.path()).unwrap_err().to_string();
        assert!(error.contains("planner failed"), "{error}");
        assert!(!dir.path().join("never-replay").exists());
    }

    #[test]
    fn forced_recipe_failure_is_a_failed_plan_even_after_side_effects() {
        let dir = fixture("all:\n\t+touch forced-result; exit 7\n");
        assert!(plan(&[], dir.path()).is_err());
        assert!(dir.path().join("forced-result").exists());
    }

    #[test]
    fn planning_flags_and_control_variables_cannot_be_overridden() {
        for arg in [
            "-C/tmp",
            "--directory=/tmp",
            "--directory",
            "--direc=/tmp",
            "-j8",
            "--jobs=8",
            "-nj8",
            "-t",
            "-q",
            "--eval=all:;touch bad",
            "--output-sync",
            "--silent",
            "--help",
            "--version",
            "MAKEFLAGS=-j8",
            "MAKEFLAGS:=-t",
            "GNUMAKEFLAGS=-C/tmp",
            "MFLAGS=-s",
            "MAKE=sh",
        ] {
            assert!(validate_args(&[arg.into()]).is_err(), "accepted {arg}");
        }
        assert!(validate_args(&["--".into(), "MAKEFLAGS=-t".into()]).is_err());
        assert!(validate_args(&["-f".into()]).is_err());
        assert!(validate_args(&["--include-dir".into()]).is_err());
    }

    #[test]
    fn supported_makefile_include_and_variable_arguments_are_forwarded() {
        assert!(
            validate_args(&[
                "-nrR".into(),
                "-fcustom.mk".into(),
                "-I".into(),
                "includes".into(),
                "--makefile=other.mk".into(),
                "CC=clang".into(),
                "CFLAGS+=-O2".into(),
                "all".into()
            ])
            .is_ok()
        );
        let dir = fixture("unused:\n\t@touch unused\n");
        fs::write(dir.path().join("custom.mk"), "all:\n\t@echo $(VALUE)\n").unwrap();
        let log = plan(
            &["-f".into(), "custom.mk".into(), "VALUE=hello".into()],
            dir.path(),
        )
        .unwrap();
        assert_eq!(log.commands[0].text, "echo hello");
        assert!(!dir.path().join("unused").exists());
    }
}
