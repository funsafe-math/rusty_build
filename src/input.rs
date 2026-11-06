use crate::shell::{self, ParsedLine};
use anyhow::{Context, Result, bail};

/// Read physical lines and join only active backslash-newline continuations.
/// Paths and command spelling are not normalized; CRLF is a record delimiter.
pub fn read_log(text: &str) -> Result<Vec<ParsedLine>> {
    let mut commands = Vec::new();
    let mut pending = String::new();
    let mut start = 1;
    let mut quote = None;
    for (index, physical) in text.split_inclusive('\n').enumerate() {
        let line_number = index + 1;
        let has_newline = physical.ends_with('\n');
        let line = physical.strip_suffix('\n').unwrap_or(physical);
        let line = if has_newline {
            line.strip_suffix('\r').unwrap_or(line)
        } else {
            line
        };
        if pending.is_empty() {
            start = line_number;
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            if metadata(line)? {
                continue;
            }
        }
        let continued = active_continuation(line, &mut quote);
        pending.push_str(line);
        if continued {
            if !has_newline {
                bail!("unfinished continuation on line {line_number}");
            }
            pending.push('\n');
            continue;
        }
        let mut command =
            shell::parse(&pending).with_context(|| format!("command on line {start}"))?;
        command.line = start;
        commands.push(command);
        pending.clear();
        quote = None;
    }
    if !pending.is_empty() {
        bail!("unfinished continuation on line {start}");
    }
    Ok(commands)
}

// Trace/diagnostic records are not commands. Directory-changing recursive Make
// output cannot be replayed without changing the meaning of relative operands.
fn metadata(line: &str) -> Result<bool> {
    let Some((prefix, message)) = line.split_once(": ") else {
        return Ok(false);
    };
    let program = prefix.rsplit('/').next().unwrap_or(prefix);
    if program == "make"
        || program == "gmake"
        || program.starts_with("make[")
        || program.starts_with("gmake[")
    {
        if message.contains("***") || message.starts_with("No rule to make target") {
            bail!("failed Make log: {line}");
        }
        if message.starts_with("Entering directory") || message.starts_with("Leaving directory") {
            bail!("directory-changing Make logs are unsupported: {line}");
        }
        return Ok(true);
    }
    let numbered = prefix
        .rsplit_once(':')
        .is_some_and(|(_, number)| number.parse::<usize>().is_ok());
    Ok(numbered
        && (message.starts_with("update target ")
            || message.starts_with("target ")
            || message.starts_with("warning:")
            || message.starts_with("note:")))
}

fn active_continuation(line: &str, quote: &mut Option<char>) -> bool {
    let mut escaped = false;
    let mut word_start = quote.is_none();
    for ch in line.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match (*quote, ch) {
            (Some('\''), '\'') => *quote = None,
            (Some('\''), _) => {}
            (Some('"'), '"') => *quote = None,
            (Some('"'), '\\') => escaped = true,
            (Some('"'), _) => {}
            (None, '#') if word_start => return false,
            (None, '\'' | '"') => {
                *quote = Some(ch);
                word_start = false;
            }
            (None, '\\') => {
                escaped = true;
                word_start = false;
            }
            _ => word_start = ch.is_whitespace() || ";&|()<>".contains(ch),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crlf_continuations_preserve_words_and_physical_lines() {
        let log = read_log("cp se\\\r\ned result\r\ncp 'a b' 'c d'\r\n").unwrap();
        assert_eq!(log[0].argv, ["cp", "seed", "result"]);
        assert_eq!(log[1].line, 3);
        assert_eq!(log[1].argv, ["cp", "a b", "c d"]);
    }

    #[test]
    fn all_lines_are_validated_before_exposing_jobs() {
        assert!(read_log("touch prefix\ncp source target | other\n").is_err());
        assert!(read_log("touch prefix\ncp \\\n").is_err());
        assert!(read_log("make[1]: Entering directory 'other'\ntouch wrong\n").is_err());
    }
}
