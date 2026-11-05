//! Conservative Bash command classification, not a shell interpreter.
//!
//! tree-sitter-bash validates the complete syntax tree before any static argv is
//! exposed. Only a single simple command with literal words is decoded using
//! shell-words. A sole `COMMAND || cd .` exposes that same static command with
//! explicit fallback metadata; its original source is never rewritten. Other
//! lists, expansions, assignments, pipelines, and valid complex Bash syntax
//! remain opaque and must run as the original text through `bash -c`.
//! This is deliberately Bash/POSIX quoting, even for Windows-looking paths:
//! unquoted `C:\\tmp` loses its escaping backslash, whereas `'C:\\tmp'` preserves
//! it. Backslashes are never converted to path separators. This does not parse
//! cmd.exe or PowerShell logs, and does not emulate their quoting on Linux.
//!
//! At most one ordinary file redirection is represented, except independent
//! stdout/stderr `/dev/null` sinks. Other multiple redirects, descriptor
//! duplication/closure, special Bash device paths, and stderr append remain
//! opaque, preserving descriptor ordering and file-opening side effects.
//!
//! Parsing is entirely in-process, on every host: tree-sitter recovery trees and
//! incomplete constructs are rejected before any argv is exposed. Syntax outside
//! the grammar's supported coverage is conservatively rejected, never delegated
//! to a shell during planning. Only opaque job execution requires Bash (such as
//! Git Bash on Windows); static typed commands execute native argv directly.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
};
use tree_sitter::{Node, Parser};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShellCommand {
    pub program: String,
    pub args: Vec<String>,
    pub stdin: Option<PathBuf>,
    pub stdout: Option<PathBuf>,
    pub stderr: Option<PathBuf>,
    pub append_stdout: bool,
    /// Original recipe is exactly this command followed by `|| cd .`.
    /// Ordinary execution must preserve its shell-level status fallback.
    #[serde(default)]
    pub noop_fallback: bool,
}

thread_local! {
    static PARSER: RefCell<Option<Parser>> = const { RefCell::new(None) };
}

/// Validate the entire command, reusing the grammar setup on this thread.
/// Return static argv only when no Bash semantics would be lost. `None` is an
/// opaque, valid shell program, never a partial parse. This starts no processes.
pub fn parse(text: &str) -> Result<Option<ShellCommand>> {
    if text.contains('\0') {
        bail!("shell command contains a NUL byte");
    }
    if dangling_escape(text) {
        bail!("incomplete Bash escape at end of command");
    }
    PARSER.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            let mut parser = Parser::new();
            parser
                .set_language(&tree_sitter_bash::LANGUAGE.into())
                .context("loading Bash grammar")?;
            *slot = Some(parser);
        }
        let tree = slot
            .as_mut()
            .expect("initialized Bash parser")
            .parse(text, None)
            .context("parsing Bash command")?;
        let root = tree.root_node();
        if root.has_error() {
            bail!("malformed, incomplete, or unsupported Bash syntax");
        }
        validate_structure(root, text)?;
        parse_static(root, text)
    })
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
    let (statement, noop_fallback) = if statements[0].kind() == "list" {
        let list = statements[0];
        // Do not flatten nested lists, pipelines, assignments, or quoted text.
        if list.child_count() != 3
            || list.named_child_count() != 2
            || list.child(1).is_none_or(|operator| operator.kind() != "||")
        {
            return Ok(None);
        }
        let left = list.named_child(0).expect("left list command");
        let right = list.named_child(1).expect("right list command");
        let mut words = Vec::new();
        let mut redirects = Vec::new();
        if !collect_simple(right, &mut words, &mut redirects)
            || !redirects.is_empty()
            || words.len() != 2
            || &text[words[0].byte_range()] != "cd"
            || &text[words[1].byte_range()] != "."
            || !complete_static_spelling(right, text)
        {
            return Ok(None);
        }
        (left, true)
    } else {
        (statements[0], false)
    };
    if !complete_static_spelling(statement, text) {
        return Ok(None);
    }

    let mut words = Vec::new();
    let mut redirects = Vec::new();
    if !collect_simple(statement, &mut words, &mut redirects) || redirects.len() > 2 {
        return Ok(None);
    }
    let mut words = words.into_iter().peekable();
    let Some(program_word) = words.next() else {
        return Ok(None);
    };
    let Some(program) = decode_command_word(program_word, &mut words, text)? else {
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
    while let Some(word) = words.next() {
        let Some(decoded) = decode_command_word(word, &mut words, text)? else {
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
        noop_fallback,
    };
    for redirect in &redirects {
        if !apply_redirect(*redirect, text, &mut command)? {
            return Ok(None);
        }
    }
    if redirects.len() == 2
        && (command.stdin.is_some()
            || command.stdout.as_deref() != Some(Path::new("/dev/null"))
            || command.stderr.as_deref() != Some(Path::new("/dev/null"))
            || command.append_stdout)
    {
        return Ok(None);
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

/// Cover grammar allowances that are not complete Bash statements. In
/// particular, reserved closing words cannot be ordinary command names, empty
/// compound bodies are invalid, and the scanner can synthesize a here-doc end
/// at EOF. These checks inspect structure, not command execution or expansion.
fn validate_structure(node: Node<'_>, text: &str) -> Result<()> {
    if node.is_missing() {
        bail!("incomplete Bash syntax");
    }
    if matches!(node.kind(), ";;" | ";&" | ";;&")
        && !node
            .parent()
            .is_some_and(|parent| parent.kind() == "case_item")
    {
        bail!("unexpected Bash case terminator");
    }
    let spelling = &text[node.start_byte()..node.end_byte()];
    if node.kind() == "command_name"
        && matches!(
            spelling,
            "then"
                | "else"
                | "elif"
                | "fi"
                | "done"
                | "do"
                | "esac"
                | "in"
                | "if"
                | "for"
                | "select"
                | "while"
                | "until"
                | "case"
                | "function"
                | "{"
                | "}"
        )
    {
        bail!("unexpected Bash reserved word {spelling}");
    }
    if node.kind() == "command" {
        let mut cursor = node.walk();
        let subshell = node
            .named_children(&mut cursor)
            .any(|child| child.kind() == "subshell");
        let name = node
            .child_by_field_name("name")
            .map(|name| &text[name.start_byte()..name.end_byte()]);
        if subshell && !matches!(name, Some("time" | "coproc")) {
            bail!("unexpected Bash subshell after command");
        }
        if name == Some("coproc") && node.named_child_count() == 1 {
            bail!("incomplete Bash coprocess");
        }
    }
    if matches!(node.kind(), "do_group" | "compound_statement")
        && (node.kind() == "do_group" || spelling.starts_with('{'))
    {
        let mut cursor = node.walk();
        if !node
            .named_children(&mut cursor)
            .any(|child| child.kind() != "comment")
        {
            bail!("empty Bash compound body");
        }
    }
    if matches!(node.kind(), "if_statement" | "elif_clause" | "else_clause") {
        let mut cursor = node.walk();
        let mut body = None;
        for child in node.children(&mut cursor) {
            match child.kind() {
                "then" | "else" => body = Some(false),
                "fi" | "elif_clause" | "else_clause" => {
                    if body == Some(false) {
                        bail!("empty Bash conditional body");
                    }
                    body = None;
                }
                "comment" => {}
                _ if child.is_named() && body.is_some() => body = Some(true),
                _ => {}
            }
        }
        if body == Some(false) {
            bail!("empty Bash conditional body");
        }
    }
    if node.kind() == "heredoc_redirect" {
        let mut cursor = node.walk();
        let start = node
            .named_children(&mut cursor)
            .find(|child| child.kind() == "heredoc_start")
            .context("incomplete Bash here-document delimiter")?;
        let mut cursor = node.walk();
        let end = node
            .named_children(&mut cursor)
            .find(|child| child.kind() == "heredoc_end")
            .context("incomplete Bash here-document")?;
        let delimiter = shell_words::split(&text[start.start_byte()..start.end_byte()])
            .context("unsupported Bash here-document delimiter")?;
        let end_text = &text[end.start_byte()..end.end_byte()];
        let suffix = &text[end.end_byte()..];
        let line_start = text[..end.start_byte()]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        let indent = &text[line_start..end.start_byte()];
        let tabs_allowed = spelling.trim_start().starts_with("<<-")
            || spelling
                .trim_start_matches(|ch: char| ch.is_ascii_digit())
                .starts_with("<<-");
        if delimiter.len() != 1
            || delimiter[0] != end_text
            || (!suffix.is_empty() && !suffix.starts_with('\n'))
            || (!indent.is_empty() && !(tabs_allowed && indent.chars().all(|ch| ch == '\t')))
        {
            bail!("incomplete Bash here-document");
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        validate_structure(child, text)?;
    }
    Ok(())
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

// The grammar emits separate word nodes across an unquoted escaped newline.
// Bash removes that pair before tokenization: `se\\\ned` is one word, not two.
fn decode_command_word<'tree>(
    word: Node<'tree>,
    following: &mut std::iter::Peekable<std::vec::IntoIter<Node<'tree>>>,
    text: &str,
) -> Result<Option<String>> {
    let mut end = word.end_byte();
    while let Some(next) = following.peek() {
        let gap = &text[end..next.start_byte()];
        if gap.is_empty() || !gap.split("\\\n").all(str::is_empty) {
            break;
        }
        end = following.next().expect("peeked word").end_byte();
    }
    decode_spelling(&text[word.start_byte()..end])
}

fn decode_static(node: Node<'_>, text: &str) -> Result<Option<String>> {
    if !static_shape(node) {
        return Ok(None);
    }
    decode_spelling(&text[node.byte_range()])
}

fn decode_spelling(raw: &str) -> Result<Option<String>> {
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
