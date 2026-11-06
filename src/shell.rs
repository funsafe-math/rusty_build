//! Only literal command words, `>`, `2>` and a recognized final no-op fallback.
//! This module extracts hints; it does not execute or represent shell programs.
use anyhow::{Context, Result, bail, ensure};
use brush_parser::{Token, TokenizerOptions, uncached_tokenize_str};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShellHints {
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub ignore_failure: bool,
    stderr_first: bool,
}

impl ShellHints {
    pub fn new(stdout: Option<String>, stderr: Option<String>, ignore_failure: bool) -> Self {
        Self {
            stdout,
            stderr,
            ignore_failure,
            stderr_first: false,
        }
    }

    pub fn redirects(&self) -> impl Iterator<Item = (Stream, &str)> {
        let stdout = (Stream::Stdout, self.stdout.as_deref());
        let stderr = (Stream::Stderr, self.stderr.as_deref());
        let ordered = if self.stderr_first {
            [stderr, stdout]
        } else {
            [stdout, stderr]
        };
        ordered
            .into_iter()
            .filter_map(|(stream, path)| path.map(|path| (stream, path)))
    }
}

#[derive(Debug)]
pub struct ParsedLine {
    pub argv: Vec<String>,
    pub shell: ShellHints,
    pub line: usize,
    pub text: String,
}

pub fn parse(text: &str) -> Result<ParsedLine> {
    ensure!(!text.contains('\0'), "NUL in command");
    let tokens =
        uncached_tokenize_str(text, &TokenizerOptions::default()).context("tokenizing command")?;
    let mut argv = Vec::new();
    let mut shell = ShellHints::default();
    let mut index = 0;
    let mut redirect_seen = false;
    while index < tokens.len() {
        let mut stream = Stream::Stdout;
        if let Token::Word(raw, span) = &tokens[index] {
            let descriptor = tokens.get(index + 1).is_some_and(|next| {
                matches!(next, Token::Operator(op, _) if op == ">")
                    && span.end.index == next.location().start.index
                    && !raw.is_empty()
                    && raw.bytes().all(|c| c.is_ascii_digit())
            });
            if descriptor {
                stream = match raw.as_str() {
                    "1" => Stream::Stdout,
                    "2" => Stream::Stderr,
                    _ => bail!("unsupported output descriptor {raw}"),
                };
                index += 1;
            } else {
                if argv.is_empty() {
                    let assignment = raw.split_once('=').is_some_and(|(name, _)| {
                        !name.is_empty()
                            && name.bytes().enumerate().all(|(index, byte)| {
                                byte == b'_'
                                    || byte.is_ascii_alphabetic()
                                    || (index != 0 && byte.is_ascii_digit())
                            })
                    });
                    ensure!(!assignment, "environment assignments are unsupported");
                }
                argv.push(decode_word(raw)?);
                index += 1;
                continue;
            }
        }
        let Token::Operator(operator, _) = &tokens[index] else {
            unreachable!()
        };
        match operator.as_str() {
            ">" => {
                index += 1;
                let Some(Token::Word(raw, _)) = tokens.get(index) else {
                    bail!("output redirect requires a filename")
                };
                let path = decode_word(raw)?;
                ensure!(
                    !path.is_empty(),
                    "output redirect requires a nonempty filename"
                );
                if !redirect_seen {
                    shell.stderr_first = stream == Stream::Stderr;
                }
                redirect_seen = true;
                let destination = match stream {
                    Stream::Stdout => &mut shell.stdout,
                    Stream::Stderr => &mut shell.stderr,
                };
                ensure!(
                    destination.is_none(),
                    "repeated redirects for one stream are unsupported"
                );
                *destination = Some(path);
                index += 1;
            }
            "||" => {
                let suffix = tokens[index + 1..]
                    .iter()
                    .map(|token| match token {
                        Token::Word(raw, _) => decode_word(raw),
                        _ => bail!("only a final no-op is supported after ||"),
                    })
                    .collect::<Result<Vec<_>>>()?;
                ensure!(
                    matches!(suffix.as_slice(), [one] if one == "true" || one == ":")
                        || matches!(suffix.as_slice(), [cd, dot] if cd == "cd" && dot == "."),
                    "only || true, || : and || cd . are supported"
                );
                shell.ignore_failure = true;
                index = tokens.len();
            }
            _ => bail!("unsupported shell operator {operator:?}"),
        }
    }
    ensure!(
        argv.first().is_some_and(|name| !name.is_empty()),
        "command has no executable"
    );
    Ok(ParsedLine {
        argv,
        shell,
        line: 0,
        text: text.to_owned(),
    })
}

fn decode_word(raw: &str) -> Result<String> {
    // Tokenization establishes boundaries; this check excludes expansion rather
    // than pretending a shell variable/glob is a literal filename.
    let mut quote = None;
    let mut chars = raw.chars();
    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (Some('\''), '\'') => quote = None,
            (Some('\''), _) => {}
            (Some('"'), '"') => quote = None,
            (Some('"'), '\\') | (None, '\\') => {
                ensure!(chars.next().is_some(), "unfinished escape");
            }
            (Some('"'), '$' | '`') => bail!("shell expansion is unsupported"),
            (Some('"'), _) => {}
            (None, '\'' | '"') => quote = Some(ch),
            (None, '$' | '`' | '*' | '?' | '[' | ']' | '{' | '}' | '~') => {
                bail!("shell expansion is unsupported")
            }
            _ => {}
        }
    }
    let mut words = shell_words::split(raw).context("decoding literal word")?;
    ensure!(words.len() == 1, "expected one literal word");
    Ok(words.pop().expect("one word"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_operators_and_paths_remain_arguments() {
        let line = parse("sed 's/foo/>/g' 'input file'> 'output file' 2>/dev/null").unwrap();
        assert_eq!(line.argv, ["sed", "s/foo/>/g", "input file"]);
        assert_eq!(line.shell.stdout.as_deref(), Some("output file"));
        assert_eq!(line.shell.stderr.as_deref(), Some("/dev/null"));
        assert_eq!(
            parse("tool '>' '||' '$name'").unwrap().argv,
            ["tool", ">", "||", "$name"]
        );
        assert_eq!(
            parse("'/opt/tools=debug/coan' source -DFEATURE=1 input.c")
                .unwrap()
                .argv[0],
            "/opt/tools=debug/coan"
        );
        assert!(parse("MODE=debug coan input.c").is_err());
    }

    #[test]
    fn descriptors_are_not_operands_and_noop_retains_failure_policy() {
        let line = parse("tool input 2>/dev/null >out || cd .").unwrap();
        assert_eq!(line.argv, ["tool", "input"]);
        assert!(line.shell.ignore_failure);
        assert_eq!(
            line.shell.redirects().collect::<Vec<_>>(),
            [(Stream::Stderr, "/dev/null"), (Stream::Stdout, "out")]
        );
        let line = parse("tool '2'>out").unwrap();
        assert_eq!(line.argv, ["tool", "2"]);
        assert!(line.shell.stderr.is_none());
    }

    #[test]
    fn unsupported_control_flow_and_expansion_are_errors() {
        for text in [
            "tool | cat",
            "tool && other",
            "tool || echo nope",
            "tool >",
            "tool >a >b",
            "tool $name",
            "tool $(touch nope)",
            "tool *.c",
            "tool >>out",
            "tool <in",
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
    }
}
