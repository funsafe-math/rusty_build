//! GNU Make dry-run log ingestion with Bash, not cmd.exe, recipe semantics.
//!
//! Physical source line numbers and command spelling are retained. Only an
//! active Bash backslash-newline joins physical lines; open quotes, here-docs,
//! and compound statements are not implicitly assembled across log records.
//! Make trace lines attach target metadata to subsequent recipes. Diagnostics
//! never become commands, and failed Make logs or directory-scoped recipes are
//! rejected before execution. CRLF record delimiters are accepted, but CRLF
//! continuations are rejected because Bash does not treat `\\\r\n` as `\\\n`.

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceCommand {
    pub line: usize,
    pub text: String,
    pub target: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceDiagnostic {
    /// None denotes a planner warning, not a physical input record.
    pub line: Option<usize>,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct InputLog {
    pub commands: Vec<SourceCommand>,
    pub diagnostics: Vec<SourceDiagnostic>,
}

static TRACE_UPDATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^.+:[0-9]+: update target ['`](.*)' due to:.*$").unwrap());
static TRACE_MISSING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^.+:[0-9]+: target ['`](.*)' does not exist$").unwrap());
static MAKE_DIAGNOSTIC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:.*[/\\])?(?:g?make)(?:\[[0-9]+\])?: (.*)$").unwrap());
static FILE_DIAGNOSTIC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^.+:[0-9]+(?::[0-9]+)?: (?:warning:|note:|\*\*\*|warning[ :]).*$").unwrap()
});

/// Read complete dry-run records; never recover by accepting a command prefix.
pub fn read_log(text: &str) -> Result<InputLog> {
    let mut log = InputLog {
        commands: Vec::new(),
        diagnostics: Vec::new(),
    };
    let mut target = None;
    let mut pending: Option<SourceCommand> = None;
    let mut quote = Quote::Unquoted;
    let mut directory_depth = 0usize;

    for (index, physical) in text.split_inclusive('\n').enumerate() {
        let line_number = index + 1;
        let has_newline = physical.ends_with('\n');
        let without_newline = physical.strip_suffix('\n').unwrap_or(physical);
        let crlf = has_newline && without_newline.ends_with('\r');
        let line = if crlf {
            &without_newline[..without_newline.len() - 1]
        } else {
            without_newline
        };
        let metadata = classify_metadata(line);
        if pending.is_some() && metadata.is_some() {
            bail!("Make metadata interrupts a shell continuation at source line {line_number}");
        }
        if let Some(metadata) = metadata {
            match metadata {
                Metadata::Trace(next_target) => target = Some(next_target),
                Metadata::Diagnostic(message) => {
                    if message.contains("***")
                        || message.starts_with("No rule to make target ")
                        || message.ends_with(" Stop.")
                    {
                        bail!("failed Make log at source line {line_number}: {line}");
                    }
                    if message.starts_with("Entering directory ") {
                        directory_depth += 1;
                        target = None;
                    } else if message.starts_with("Leaving directory ") {
                        directory_depth = directory_depth.saturating_sub(1);
                        target = None;
                    }
                }
            }
            log.diagnostics.push(SourceDiagnostic {
                line: Some(line_number),
                text: line.to_owned(),
            });
            continue;
        }
        if pending.is_none() && (line.trim().is_empty() || line.trim_start().starts_with('#')) {
            continue;
        }
        if directory_depth != 0 {
            bail!(
                "directory-scoped Make recipe at source line {line_number} cannot be replayed safely: logs do not carry a command working directory"
            );
        }
        let continued = active_continuation(line, &mut quote);
        if continued && crlf {
            bail!(
                "CRLF shell continuation at source line {line_number} is not a Bash backslash-newline"
            );
        }
        let mut command = pending.take().unwrap_or_else(|| SourceCommand {
            line: line_number,
            text: String::new(),
            target: target.clone(),
        });
        command.text.push_str(line);
        if continued {
            if !has_newline {
                bail!("incomplete shell continuation at source line {line_number}");
            }
            command.text.push('\n');
            pending = Some(command);
            continue;
        }
        crate::shell::parse(&command.text)
            .with_context(|| format!("invalid shell command at source line {}", command.line))?;
        log.commands.push(command);
        quote = Quote::Unquoted;
    }
    if let Some(command) = pending {
        bail!(
            "incomplete shell continuation starting at source line {}",
            command.line
        );
    }
    Ok(log)
}

enum Metadata<'a> {
    Trace(String),
    Diagnostic(&'a str),
}

fn classify_metadata(line: &str) -> Option<Metadata<'_>> {
    for pattern in [&*TRACE_UPDATE, &*TRACE_MISSING] {
        if let Some(captures) = pattern.captures(line) {
            return Some(Metadata::Trace(captures[1].to_owned()));
        }
    }
    if let Some(captures) = MAKE_DIAGNOSTIC.captures(line) {
        return Some(Metadata::Diagnostic(captures.get(1).unwrap().as_str()));
    }
    if FILE_DIAGNOSTIC.is_match(line) {
        return Some(Metadata::Diagnostic(line));
    }
    None
}

#[derive(Clone, Copy)]
enum Quote {
    Unquoted,
    Single,
    Double,
}

/// Return whether the final backslash escapes the physical line boundary.
/// Comments and single quotes make backslashes literal. Quote state crosses a
/// record boundary only when that boundary is an explicit continuation.
fn active_continuation(line: &str, quote: &mut Quote) -> bool {
    let mut escaped = false;
    let mut word_start = matches!(quote, Quote::Unquoted);
    for ch in line.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match quote {
            Quote::Single => {
                if ch == '\'' {
                    *quote = Quote::Unquoted;
                }
            }
            Quote::Double => match ch {
                '"' => *quote = Quote::Unquoted,
                '\\' => escaped = true,
                _ => {}
            },
            Quote::Unquoted => match ch {
                '#' if word_start => return false,
                '\'' => {
                    *quote = Quote::Single;
                    word_start = false;
                }
                '"' => {
                    *quote = Quote::Double;
                    word_start = false;
                }
                '\\' => {
                    escaped = true;
                    word_start = false;
                }
                _ => word_start = ch.is_whitespace() || ";&|()<>".contains(ch),
            },
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_plain_commands_with_exact_source_locations() {
        let log = read_log("\n# build\n  cc -c a.c -o a.o\nprintf '%s' 'done'\n").unwrap();
        assert_eq!(log.commands.len(), 2);
        assert_eq!(log.commands[0].line, 3);
        assert_eq!(log.commands[0].text, "  cc -c a.c -o a.o");
        assert_eq!(log.commands[1].line, 4);
        assert!(log.commands[0].target.is_none());
    }

    #[test]
    fn reads_representative_gnu_make_trace() {
        let text = "Makefile:4: update target 'build/a.o' due to: src/a.c include/a.h\ncc -Iinclude -c src/a.c -o build/a.o\nMakefile:7: target 'all' does not exist\nprintf '%s\\n' done\nmake: Nothing to be done for 'extra'.\n";
        let log = read_log(text).unwrap();
        assert_eq!(log.commands.len(), 2);
        assert_eq!(log.commands[0].line, 2);
        assert_eq!(log.commands[0].target.as_deref(), Some("build/a.o"));
        assert_eq!(log.commands[1].target.as_deref(), Some("all"));
        assert_eq!(log.diagnostics.len(), 3);
        assert_eq!(log.diagnostics[0].text, text.lines().next().unwrap());
        assert_eq!(
            log.diagnostics.iter().map(|d| d.line).collect::<Vec<_>>(),
            vec![Some(1), Some(3), Some(5)]
        );
    }

    #[test]
    fn trace_target_handles_spaces_apostrophes_and_windows_drive_prefix() {
        let log = read_log(
            "C:/work/Makefile:123: update target 'it's a target.o' due to: a.c\ncc -c a.c\n",
        )
        .unwrap();
        assert_eq!(log.commands[0].target.as_deref(), Some("it's a target.o"));
        let log = read_log("Makefile:1: target `old quoting' does not exist\necho ok\n").unwrap();
        assert_eq!(log.commands[0].target.as_deref(), Some("old quoting"));
    }

    #[test]
    fn diagnostics_are_not_shell_commands() {
        let log = read_log("make: 'all' is up to date.\nmake[1]: Nothing to be done for 'all'.\nMakefile:4: warning: overriding recipe for target 'x'\nMakefile:3: warning: ignoring old recipe for target 'x'\n/usr/bin/gmake: Nothing to be done for 'all'.\n").unwrap();
        assert!(log.commands.is_empty());
        assert_eq!(log.diagnostics.len(), 5);
        assert!(
            read_log(
                "make[1]: Entering directory '/tmp/sub'\nmake[1]: Leaving directory '/tmp/sub'\n"
            )
            .unwrap()
            .commands
            .is_empty()
        );
    }

    #[test]
    fn rejects_failed_make_logs_and_directory_scoped_recipes() {
        for text in [
            "echo partial\nmake: *** [Makefile:2: all] Error 1\n",
            "make: *** No rule to make target 'missing'. Stop.\n",
            "Makefile:2: *** missing separator. Stop.\n",
            "make[1]: Entering directory '/tmp/sub'\nMakefile:1: target 'x' does not exist\necho wrong-cwd\n",
        ] {
            assert!(read_log(text).is_err(), "must reject unsafe log: {text}");
        }
    }

    #[test]
    fn preserves_logical_continuations_and_starting_line() {
        let text = "Makefile:2: target 'x' does not exist\ncc -c \\\n  'source file.c' \\\n  -o out.o\necho done\n";
        let log = read_log(text).unwrap();
        assert_eq!(log.commands.len(), 2);
        assert_eq!(log.commands[0].line, 2);
        assert_eq!(
            log.commands[0].text,
            "cc -c \\\n  'source file.c' \\\n  -o out.o"
        );
        assert_eq!(log.commands[0].target.as_deref(), Some("x"));
        assert_eq!(log.commands[1].line, 5);
        let log = read_log("echo \"ab\\\ncd\"\n").unwrap();
        assert_eq!(log.commands[0].text, "echo \"ab\\\ncd\"");
    }

    #[test]
    fn continuation_detection_respects_quotes_comments_and_even_escapes() {
        let log = read_log("echo 'literal\\'\necho ok # comment\\\necho last\n").unwrap();
        assert_eq!(log.commands.len(), 3);
        let log = read_log("echo two\\\\\necho next\n").unwrap();
        assert_eq!(log.commands.len(), 2);
        let log = read_log("echo foo#not-comment\\\nbar\n").unwrap();
        assert_eq!(log.commands.len(), 1);
    }

    #[test]
    fn rejects_implicit_multiline_and_unfinished_continuations() {
        for text in [
            "echo 'first\nsecond'\n",
            "echo \"first\nsecond\"\n",
            "cat <<EOF\nbody\nEOF\n",
            "if true; then\necho yes\nfi\n",
            "echo x &&\necho y\n",
            "echo trailing\\",
            "echo trailing\\\n",
            "echo x \\\nMakefile:2: target 'next' does not exist\necho next\n",
            "echo x \\\r\ny\r\n",
            "echo \0bad\n",
        ] {
            assert!(
                read_log(text).is_err(),
                "must reject unsafe multiline log: {text:?}"
            );
        }
    }

    #[test]
    fn valid_complex_single_line_is_preserved_for_bash() {
        let log =
            read_log("cd build && cc -c a.c -o a.o\nfor x in a b; do echo \"$x\"; done\n").unwrap();
        assert_eq!(log.commands.len(), 2);
        assert_eq!(log.commands[0].text, "cd build && cc -c a.c -o a.o");
        assert_eq!(log.commands[1].line, 2);
    }

    #[test]
    fn crlf_records_do_not_change_windows_path_spelling() {
        let text = "tool 'C:\\work\\foo.c'\r\ntool C:\\work\\foo.c\r\n";
        let log = read_log(text).unwrap();
        assert_eq!(log.commands[0].text, "tool 'C:\\work\\foo.c'");
        assert_eq!(log.commands[1].text, "tool C:\\work\\foo.c");
        assert_eq!(
            crate::shell::parse(&log.commands[0].text)
                .unwrap()
                .unwrap()
                .args,
            [r"C:\work\foo.c"]
        );
        assert_eq!(
            crate::shell::parse(&log.commands[1].text)
                .unwrap()
                .unwrap()
                .args,
            ["C:workfoo.c"]
        );
    }
}
