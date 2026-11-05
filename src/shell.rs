//! Conservative Bash command classification, not a shell interpreter.
//!
//! tree-sitter-bash validates the complete syntax tree before any static argv is
//! exposed. Only a single simple command with literal words is decoded using
//! shell-words; expansions, assignments, lists, pipelines, and other valid Bash
//! syntax remain opaque and must run as the original text through `bash -c`.
//! This is deliberately Bash/POSIX quoting, even for Windows-looking paths:
//! unquoted `C:\\tmp` loses its escaping backslash, whereas `'C:\\tmp'` preserves
//! it. Backslashes are never converted to path separators. This does not parse
//! cmd.exe or PowerShell logs, and does not emulate their quoting on Linux.
//!
//! At most one ordinary file redirection is represented. Multiple redirects,
//! descriptor duplication/closure, special Bash device paths, and stderr append
//! remain opaque, preserving descriptor ordering and file-opening side effects.
//!
//! Because tree-sitter's error-recovering grammar is more permissive than Bash,
//! opaque programs additionally use `bash --noprofile --norc -n -c` for syntax-only
//! validation, with BASH_ENV/ENV removed. This subprocess cost is restricted to
//! fallback programs; static commands need no subprocess. A Bash-capable Unix
//! host is required; Windows-looking arguments do not imply Windows execution.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tree_sitter::Node;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShellCommand {
    pub program: String,
    pub args: Vec<String>,
    pub stdin: Option<PathBuf>,
    pub stdout: Option<PathBuf>,
    pub stderr: Option<PathBuf>,
    pub append_stdout: bool,
}

/// Validate the entire command; return static argv only when no Bash semantics
/// would be lost. `None` is an opaque, valid shell program, never a partial parse.
pub fn parse(text: &str) -> Result<Option<ShellCommand>> {
    if !cfg!(unix) {
        bail!(
            "Bash command parsing/execution requires a Unix host; Windows shells are unsupported"
        );
    }
    if text.contains('\0') {
        bail!("shell command contains a NUL byte");
    }
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .context("loading Bash grammar")?;
    let tree = parser.parse(text, None).context("parsing Bash command")?;
    let root = tree.root_node();
    // The grammar accepts a terminal escape as a word in some contexts. Bash
    // instead needs more input; do not silently execute an incomplete log line.
    if dangling_escape(text) {
        bail!("incomplete Bash escape at end of command");
    }
    if root.has_error() || has_missing(root) {
        // A recovery tree may represent either malformed Bash or valid syntax
        // outside the grammar's coverage. Never expose argv from such a tree.
        validate_opaque(text)?;
        return Ok(None);
    }
    let command = parse_static(root, text)?;
    if command.is_none() {
        validate_opaque(text)?;
    }
    Ok(command)
}

fn parse_static(root: Node<'_>, text: &str) -> Result<Option<ShellCommand>> {
    // Bash treats only space/tab/newline as shell separators, unlike the
    // grammar's broader whitespace extras. Preserve all other spelling opaquely.
    if text
        .chars()
        .any(|ch| ch.is_whitespace() && !matches!(ch, ' ' | '\t' | '\n'))
        || !separator_gap(&text[..root.start_byte()])
        || !separator_gap(&text[root.end_byte()..])
        || !complete_static_spelling(root, text)
    {
        return Ok(None);
    }
    let mut cursor = root.walk();
    let statements: Vec<_> = root
        .named_children(&mut cursor)
        .filter(|node| node.kind() != "comment")
        .collect();
    if statements.len() != 1 {
        return Ok(None);
    }
    // Unnamed root children include list terminators, notably asynchronous `&`.
    // Even a single command followed by `;` stays opaque rather than being
    // mistaken for the executable statement alone.
    let mut cursor = root.walk();
    if root.children(&mut cursor).any(|node| !node.is_named()) {
        return Ok(None);
    }

    let mut words = Vec::new();
    let mut redirects = Vec::new();
    if !collect_simple(statements[0], &mut words, &mut redirects) || redirects.len() > 1 {
        return Ok(None);
    }
    let mut words = words.into_iter();
    let Some(program_word) = words.next() else {
        return Ok(None);
    };
    let Some(program) = decode_static(program_word, text)? else {
        return Ok(None);
    };
    if program.is_empty() {
        return Ok(None);
    }
    // These Bash reserved words are not all distinguished by the grammar.
    // In particular `time` and `coproc` must not become executable argv.
    if matches!(
        program.as_str(),
        "time"
            | "coproc"
            | "if"
            | "then"
            | "else"
            | "elif"
            | "fi"
            | "case"
            | "esac"
            | "for"
            | "select"
            | "while"
            | "until"
            | "do"
            | "done"
            | "in"
            | "function"
            | "!"
            | "{"
            | "}"
    ) {
        return Ok(None);
    }
    let mut args = Vec::with_capacity(words.len());
    for word in words {
        let Some(decoded) = decode_static(word, text)? else {
            return Ok(None);
        };
        args.push(decoded);
    }
    let mut command = ShellCommand {
        program,
        args,
        stdin: None,
        stdout: None,
        stderr: None,
        append_stdout: false,
    };
    if let Some(redirect) = redirects.first() {
        if !apply_redirect(*redirect, text, &mut command)? {
            return Ok(None);
        }
    }
    Ok(Some(command))
}

fn complete_static_spelling(node: Node<'_>, text: &str) -> bool {
    if !matches!(node.kind(), "program" | "command" | "redirected_statement") {
        return true;
    }
    let mut end = node.start_byte();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !separator_gap(&text[end..child.start_byte()]) || !complete_static_spelling(child, text)
        {
            return false;
        }
        end = child.end_byte();
    }
    separator_gap(&text[end..node.end_byte()])
}

fn separator_gap(text: &str) -> bool {
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        match ch {
            ' ' | '\t' | '\n' => {}
            '\\' if chars.next() == Some('\n') => {}
            _ => return false,
        }
    }
    true
}

fn validate_opaque(text: &str) -> Result<()> {
    let output = std::process::Command::new("bash")
        .args(["--noprofile", "--norc", "-n", "-c", text])
        .env_remove("BASH_ENV")
        .env_remove("ENV")
        .output()
        .context("checking opaque Bash syntax (bash must be installed)")?;
    // Bash reports an unterminated here-document as a warning with exit zero.
    // It is still an incomplete log record and must never be replayed.
    if !output.status.success() || !output.stderr.is_empty() {
        bail!(
            "malformed or incomplete Bash command: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn has_missing(node: Node<'_>) -> bool {
    if node.is_missing() {
        return true;
    }
    let mut cursor = node.walk();
    node.children(&mut cursor).any(has_missing)
}

fn collect_simple<'tree>(
    node: Node<'tree>,
    words: &mut Vec<Node<'tree>>,
    redirects: &mut Vec<Node<'tree>>,
) -> bool {
    match node.kind() {
        "redirected_statement" => {
            let Some(body) = node.child_by_field_name("body") else {
                return false;
            };
            if !collect_simple(body, words, redirects) {
                return false;
            }
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.id() == body.id() || child.kind() == "comment" {
                    continue;
                }
                if child.kind() != "file_redirect" {
                    return false;
                }
                redirects.push(child);
            }
            true
        }
        "command" => {
            let Some(name) = node.child_by_field_name("name") else {
                return false;
            };
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "comment" {
                    continue;
                }
                if child.kind() == "file_redirect" {
                    redirects.push(child);
                } else if child.id() == name.id() || static_shape(child) {
                    // Assignment/subshell/herestring nodes are not literal words.
                    if !static_shape(child) {
                        return false;
                    }
                    words.push(child);
                } else {
                    return false;
                }
            }
            true
        }
        _ => false,
    }
}

fn static_shape(node: Node<'_>) -> bool {
    match node.kind() {
        "word" | "raw_string" | "string_content" | "number" => true,
        "command_name" | "concatenation" | "string" => {
            let mut cursor = node.walk();
            node.named_children(&mut cursor).all(static_shape)
        }
        _ => false,
    }
}

fn decode_static(node: Node<'_>, text: &str) -> Result<Option<String>> {
    if !static_shape(node) {
        return Ok(None);
    }
    let raw = &text[node.byte_range()];
    if !literal_spelling(raw) {
        return Ok(None);
    }
    // Decode each complete AST word separately. Never split raw command text,
    // which would turn redirection operators into argv or flatten shell lists.
    let mut decoded = shell_words::split(raw).context("decoding static Bash word")?;
    if decoded.len() != 1 {
        return Ok(None);
    }
    Ok(decoded.pop())
}

/// Extra lexical conservatism covers expansions represented as plain `word`
/// tokens by the grammar (notably globs, tilde, and brace expansion).
fn literal_spelling(raw: &str) -> bool {
    let mut chars = raw.chars();
    let mut quote = None;
    while let Some(ch) = chars.next() {
        match quote {
            Some('\'') => {
                if ch == '\'' {
                    quote = None;
                }
            }
            Some('"') => match ch {
                '"' => quote = None,
                '\\' => {
                    if chars.next().is_none() {
                        return false;
                    }
                }
                '$' | '`' => return false,
                _ => {}
            },
            None => match ch {
                '\'' | '"' => quote = Some(ch),
                '\\' => {
                    if chars.next().is_none() {
                        return false;
                    }
                }
                '$' | '`' | '*' | '?' | '[' | ']' | '{' | '}' | '~' => return false,
                _ => {}
            },
            _ => unreachable!(),
        }
    }
    quote.is_none()
}

fn apply_redirect(node: Node<'_>, text: &str, command: &mut ShellCommand) -> Result<bool> {
    let mut cursor = node.walk();
    let destinations: Vec<_> = node
        .children_by_field_name("destination", &mut cursor)
        .collect();
    // The Bash grammar can include further arguments inside a redirect node.
    // Do not guess which pieces it intended to consume.
    if destinations.len() != 1 {
        return Ok(false);
    }
    let destination = destinations[0];
    let Some(path) = decode_static(destination, text)? else {
        return Ok(false);
    };
    if path.is_empty()
        || path.starts_with("/dev/fd/")
        || path.starts_with("/dev/tcp/")
        || path.starts_with("/dev/udp/")
        || matches!(path.as_str(), "/dev/stdin" | "/dev/stdout" | "/dev/stderr")
    {
        return Ok(false);
    }
    let descriptor = node
        .child_by_field_name("descriptor")
        .map(|part| &text[part.byte_range()]);
    // tree-sitter-bash 0.25 parses `0<input` as a numeric argv word plus
    // redirection, not an IO_NUMBER. Keep the original shell program opaque.
    if descriptor.is_none()
        && text
            .as_bytes()
            .get(node.start_byte().wrapping_sub(1))
            .is_some_and(u8::is_ascii_digit)
    {
        return Ok(false);
    }
    let start = node
        .child_by_field_name("descriptor")
        .map_or(node.start_byte(), |part| part.end_byte());
    let operator = text[start..destination.start_byte()].trim();
    match (descriptor, operator) {
        (None | Some("0"), "<") => command.stdin = Some(path.into()),
        (None | Some("1"), ">") => command.stdout = Some(path.into()),
        (None | Some("1"), ">>") => {
            command.stdout = Some(path.into());
            command.append_stdout = true;
        }
        (Some("2"), ">") => command.stderr = Some(path.into()),
        _ => return Ok(false),
    }
    Ok(true)
}

fn dangling_escape(text: &str) -> bool {
    let mut quote = None;
    let mut escaped = false;
    let mut comment = false;
    let mut word_start = true;
    for ch in text.chars() {
        if comment {
            if ch == '\n' {
                comment = false;
                word_start = true;
            }
            continue;
        }
        if escaped {
            escaped = false;
            continue;
        }
        match quote {
            Some('\'') => {
                if ch == '\'' {
                    quote = None;
                }
            }
            Some('"') => match ch {
                '"' => quote = None,
                '\\' => escaped = true,
                _ => {}
            },
            None => match ch {
                '#' if word_start => comment = true,
                '\'' | '"' => {
                    quote = Some(ch);
                    word_start = false;
                }
                '\\' => {
                    escaped = true;
                    word_start = false;
                }
                _ => word_start = ch.is_whitespace() || ";&|()<>".contains(ch),
            },
            _ => unreachable!(),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn simple(text: &str) -> ShellCommand {
        parse(text)
            .unwrap()
            .unwrap_or_else(|| panic!("expected static command: {text}"))
    }

    #[test]
    fn parses_compiler_and_concatenated_quoted_words() {
        let command = simple(
            r#"/usr/bin/clang++ -I'include dir' -DNAME=\"hello\" -c "source file.cc" -o obj/file.o"#,
        );
        assert_eq!(command.program, "/usr/bin/clang++");
        assert_eq!(
            command.args,
            [
                "-Iinclude dir",
                "-DNAME=\"hello\"",
                "-c",
                "source file.cc",
                "-o",
                "obj/file.o"
            ]
        );
        assert_eq!(
            simple("printf '%s' '' pre\"mid dle\"post").args,
            ["%s", "", "premid dlepost"]
        );
    }

    #[test]
    fn quoted_shell_characters_are_literal() {
        let command = simple(r#"printf '%s' '$HOME' "a*b?[x]{y}~" '&&' '|' '>' 'a#b'"#);
        assert_eq!(
            command.args,
            ["%s", "$HOME", "a*b?[x]{y}~", "&&", "|", ">", "a#b"]
        );
        assert_eq!(
            simple(r"echo a\ b \* \$HOME \~").args,
            ["a b", "*", "$HOME", "~"]
        );
        assert_eq!(
            simple("echo a#b # comment with a trailing \\").args,
            ["a#b"]
        );
    }

    #[test]
    fn windows_paths_obey_posix_escaping() {
        assert_eq!(simple(r"tool C:\work\foo.c").args, ["C:workfoo.c"]);
        assert_eq!(simple(r"tool 'C:\work\foo.c'").args, [r"C:\work\foo.c"]);
        assert_eq!(simple(r#"tool "C:\work\foo.c""#).args, [r"C:\work\foo.c"]);
        assert_eq!(simple(r"tool C:\\work\\foo.c").args, [r"C:\work\foo.c"]);
        assert_eq!(
            simple(r"tool 'C:\a b\foo.c' a/b.c").args,
            [r"C:\a b\foo.c", "a/b.c"]
        );
        assert_eq!(
            simple(r#""C:\Program Files\LLVM\clang.exe" -c 'C:\src\main.c'"#).program,
            r"C:\Program Files\LLVM\clang.exe"
        );
    }

    #[test]
    fn double_quoted_backslashes_follow_bash_rules() {
        assert_eq!(
            simple(r#"echo "a\qb" "a\\b" "\$HOME" "\`x\`""#).args,
            [r"a\qb", r"a\b", "$HOME", "`x`"]
        );
    }

    #[test]
    fn separates_one_static_redirection() {
        let command = simple("cat < 'input file'");
        assert_eq!(command.stdin, Some(PathBuf::from("input file")));
        assert!(command.args.is_empty());
        let command = simple("echo hello >out");
        assert_eq!(command.args, ["hello"]);
        assert_eq!(command.stdout, Some(PathBuf::from("out")));
        assert!(!command.append_stdout);
        let command = simple("echo hello 1>> 'out file'");
        assert_eq!(command.stdout, Some(PathBuf::from("out file")));
        assert!(command.append_stdout);
        assert_eq!(
            simple("tool 2>error.log").stderr,
            Some(PathBuf::from("error.log"))
        );
        assert!(simple("echo '>'").stdout.is_none());
    }

    #[test]
    fn valid_dynamic_and_complex_programs_are_opaque() {
        for text in [
            "echo $HOME",
            "echo ${HOME:-x}",
            "echo \"$HOME\"",
            "echo $(pwd)",
            "echo `pwd`",
            "echo $((1+2))",
            "echo *.c",
            "echo file?.c",
            "echo [ab].c",
            "echo {a,b}",
            "echo ~",
            "echo a~b",
            "VAR=value tool arg",
            "VAR=value",
            "cd build && cc -c a.c",
            "false || echo yes",
            "cat a | sort",
            "echo x; echo y",
            "echo x &",
            "echo x;",
            "(echo x)",
            "{ echo x; }",
            "! false",
            "if true; then echo yes; fi",
            "for x in a b; do echo $x; done",
            "f() { echo x; }",
            "echo <(cat a)",
            "echo $'a\\nb'",
            "echo $\"translated\"",
            "cat <<EOF\nhello\nEOF\n",
            "cat <<< hello",
            "cat >out 2>err",
            "cat 2>&1",
            "cat 2>>err",
            "cat &>out",
            "cat 3>out",
            "cat >|out",
            "cat >&-",
            "cat >/dev/tcp/localhost/80",
            "cat >$OUT",
            "cat >*.out",
            "echo x\necho y",
            "# only a comment",
            "",
            "   ",
            "time echo x",
            "coproc echo x",
            "case x in x) echo yes;; esac",
            "echo foo\r",
            "echo foo\u{b}bar",
            "time { echo x; }",
            "coproc NAME { echo x; }",
            "((x=1))",
        ] {
            assert!(
                parse(text)
                    .unwrap_or_else(|e| panic!("valid Bash rejected {text:?}: {e:#}"))
                    .is_none(),
                "must remain opaque: {text}"
            );
        }
    }

    #[test]
    fn malformed_commands_never_return_partial_argv() {
        for text in [
            "echo 'unterminated",
            "echo \"unterminated",
            "echo x &&",
            "echo x |",
            "cat >",
            "echo $(",
            "if true; then echo x",
            "echo trailing\\",
            "echo \0bad",
            "echo ;;",
            "then",
            "done",
            "cat <<EOF",
        ] {
            assert!(
                parse(text).is_err(),
                "must reject malformed shell: {text:?}"
            );
        }
    }

    #[test]
    fn explicit_newline_continuations_preserve_arguments() {
        let command = simple("cc -c \\\n 'source file.c' -o \\\n out.o");
        assert_eq!(command.args, ["-c", "source file.c", "-o", "out.o"]);
        assert_eq!(simple("echo \"ab\\\ncd\"").args, ["abcd"]);
    }

    #[test]
    fn syntax_check_never_executes_opaque_program() {
        let directory = tempfile::tempdir().unwrap();
        let sentinel = directory.path().join("must-not-exist");
        let command = format!(
            "echo $(touch {})",
            shell_words::quote(sentinel.to_str().unwrap())
        );
        assert!(parse(&command).unwrap().is_none());
        assert!(!sentinel.exists());
    }

    #[test]
    fn grammar_extras_never_silently_drop_escaped_spaces() {
        // Some grammar versions represent leading escaped spaces as extras.
        // Either an exact static argv or the original opaque program is safe.
        if let Some(command) = parse(r"echo \ leading").unwrap() {
            assert_eq!(command.args, [" leading"]);
        }
    }
}
