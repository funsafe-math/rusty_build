#![cfg(target_os = "linux")]

use rusty_make::{
    commands::compiler::Compiler, compdb::CompileCommand, graph::BuildGraph, registry::Registry,
};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::{Command, Output},
};

const CUSTOM_COMPILER: &str = "./tool bin/odd driver";
const SOURCE: &str = "source files/main unit.c";
const OBJECT: &str = "object files/main unit.o";
const APP: &str = "program files/custom app";

fn installed_compiler() -> Option<PathBuf> {
    let search_path = std::env::var_os("PATH")?;
    for name in ["cc", "gcc"] {
        for directory in std::env::split_paths(&search_path) {
            let candidate = directory.join(name);
            if fs::metadata(&candidate).is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            }) {
                return Some(candidate.canonicalize().unwrap());
            }
        }
    }
    None
}

fn compiler_fixture() -> Option<tempfile::TempDir> {
    let Some(compiler) = installed_compiler() else {
        eprintln!("skipping real compiler fixture: neither cc nor gcc is installed");
        return None;
    };
    let root = tempfile::tempdir().unwrap();
    for directory in ["tool bin", "source files", "object files", "program files"] {
        fs::create_dir(root.path().join(directory)).unwrap();
    }
    symlink(compiler, root.path().join(CUSTOM_COMPILER)).unwrap();
    write_program(root.path(), "custom compiler output");
    Some(root)
}

fn write_program(root: &Path, message: &str) {
    fs::write(
        root.join(SOURCE),
        format!("#include <stdio.h>\nint main(void) {{ puts(\"{message}\"); return 0; }}\n"),
    )
    .unwrap();
}

fn compiler_pattern() -> String {
    format!(
        r"^{}(?:\s|$)",
        regex::escape(&format!("'{CUSTOM_COMPILER}'"))
    )
}

fn cli(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rusty_make"));
    command.current_dir(root).args(["-j4", "--no-optimize"]);
    command
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_program(root: &Path, executable: &str, expected: &[u8]) {
    let output = Command::new(root.join(executable)).output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, expected);
    assert!(output.stderr.is_empty());
}

fn load_plan(root: &Path) -> BuildGraph {
    BuildGraph::from_json(
        &fs::read(root.join("plan.json")).unwrap(),
        &Registry::default(),
    )
    .unwrap()
}

fn dot_label(dot: &str, node: usize) -> String {
    let prefix = format!("  n{node} [label=");
    let encoded = dot
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .unwrap()
        .strip_suffix("];")
        .unwrap();
    serde_json::from_str(encoded).unwrap()
}

#[test]
fn custom_compiler_cli_executes_original_spaced_path_and_exports_original_identity() {
    let Some(root) = compiler_fixture() else {
        return;
    };
    let p = root.path();
    let compile = format!("'{CUSTOM_COMPILER}' -c '{SOURCE}' -o '{OBJECT}'");
    let link = format!("'{CUSTOM_COMPILER}' '{OBJECT}' -o '{APP}'");
    fs::write(
        p.join("commands.txt"),
        format!(
            "# original Make log\nMakefile:7: update target '{OBJECT}' due to: {SOURCE}\n{compile}\nMakefile:10: update target '{APP}' due to: {OBJECT}\n{link}\n"
        ),
    )
    .unwrap();
    let output = cli(p)
        .args([
            "--commands",
            "commands.txt",
            "--compiler-regex",
            &compiler_pattern(),
            "--dump-plan",
            "plan.json",
            "--dump-graph",
            "graph.dot",
            "--compile-commands",
            "compile_commands.json",
        ])
        .output()
        .unwrap();
    assert_success(&output);
    assert!(p.join(OBJECT).is_file());
    assert_program(p, APP, b"custom compiler output\n");

    let graph = load_plan(p);
    assert_eq!(graph.jobs.len(), 2);
    assert_eq!(graph.jobs[0].spec.kind, "compile");
    assert_eq!(graph.jobs[1].spec.kind, "link");
    let compile_argv = vec![CUSTOM_COMPILER, "-c", SOURCE, "-o", OBJECT];
    let link_argv = vec![CUSTOM_COMPILER, OBJECT, "-o", APP];
    for (id, (text, line, target, argv)) in [
        (&compile, 3, OBJECT, &compile_argv),
        (&link, 5, APP, &link_argv),
    ]
    .into_iter()
    .enumerate()
    {
        let job = &graph.jobs[id];
        let compiler = job.typed::<Compiler>().unwrap();
        assert_eq!(&compiler.argv, argv);
        assert_eq!(job.shell.as_ref().unwrap().program, CUSTOM_COMPILER);
        assert_eq!(&job.source.text, text);
        assert_eq!(job.source.line, line);
        assert_eq!(job.source.target.as_deref(), Some(target));
        assert_eq!(graph.provenance[id].len(), 1);
        assert_eq!(graph.provenance[id][0].source.text, *text);
        assert_eq!(graph.provenance[id][0].source.line, line);
        assert_eq!(
            graph.provenance[id][0].source.target.as_deref(),
            Some(target)
        );
    }
    assert_eq!(
        graph.jobs[0].typed::<Compiler>().unwrap().sources,
        vec![PathBuf::from(SOURCE)]
    );
    assert!(
        graph
            .edges
            .iter()
            .any(|edge| edge.from == 0 && edge.to == 1)
    );

    let entries: Vec<CompileCommand> =
        serde_json::from_slice(&fs::read(p.join("compile_commands.json")).unwrap()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].arguments, compile_argv);
    assert_eq!(entries[0].file, SOURCE);
    assert_eq!(entries[0].output.as_deref(), Some(OBJECT));
    assert_eq!(Path::new(&entries[0].directory), p.canonicalize().unwrap());
    let dot = fs::read_to_string(p.join("graph.dot")).unwrap();
    assert!(dot_label(&dot, 0).ends_with(&compile));
    assert!(dot_label(&dot, 1).ends_with(&link));
    assert!(dot.contains("n0 -> n1"));
}

#[test]
fn unconfigured_custom_compiler_is_an_uncached_barrier_despite_output_flags() {
    let Some(root) = compiler_fixture() else {
        return;
    };
    let p = root.path();
    let command = format!("'{CUSTOM_COMPILER}' '{SOURCE}' -o '{APP}'");
    fs::write(
        p.join("commands.txt"),
        format!("{command}\ncp '{APP}' delivered\n"),
    )
    .unwrap();
    for message in ["first fallback run", "changed fallback source"] {
        write_program(p, message);
        let output = cli(p)
            .args([
                "--commands",
                "commands.txt",
                "--dump-plan",
                "plan.json",
                "--compile-commands",
                "compile_commands.json",
            ])
            .output()
            .unwrap();
        assert_success(&output);
        assert_program(p, APP, format!("{message}\n").as_bytes());
        assert_program(p, "delivered", format!("{message}\n").as_bytes());
        let graph = load_plan(p);
        let unknown = &graph.jobs[0];
        assert_eq!(unknown.spec.kind, "unknown");
        assert!(unknown.spec.unknown_effects);
        assert!(!unknown.spec.cacheable);
        assert!(unknown.spec.inputs.is_empty());
        assert!(unknown.spec.outputs.is_empty());
        assert_eq!(unknown.source.text, command);
        assert!(
            graph
                .edges
                .iter()
                .any(|edge| edge.from == 0 && edge.to == 1)
        );
        let entries: Vec<CompileCommand> =
            serde_json::from_slice(&fs::read(p.join("compile_commands.json")).unwrap()).unwrap();
        assert!(entries.is_empty());
    }
}

fn assert_rejected_without_effects(root: &Path, output: &Output, forbidden: &[&str]) {
    assert!(!output.status.success());
    for path in forbidden {
        assert!(!root.join(path).exists(), "unexpected side effect: {path}");
    }
    assert!(!root.join(".rusty_make").exists());
}

#[test]
fn overlapping_custom_compiler_patterns_reject_entire_log_before_prefix_copy() {
    let Some(root) = compiler_fixture() else {
        return;
    };
    let p = root.path();
    fs::write(p.join("prefix-input"), "must not be copied\n").unwrap();
    fs::write(
        p.join("commands.txt"),
        format!(
            "cp prefix-input prefix-written\n'{CUSTOM_COMPILER}' -c '{SOURCE}' -o '{OBJECT}'\n"
        ),
    )
    .unwrap();
    let exact = compiler_pattern();
    let broader = format!(r"^{}", regex::escape(&format!("'{CUSTOM_COMPILER}'")));
    let output = cli(p)
        .args([
            "--commands",
            "commands.txt",
            "--compiler-regex",
            &exact,
            "--compiler-regex",
            &broader,
        ])
        .output()
        .unwrap();
    assert_rejected_without_effects(p, &output, &["prefix-written", OBJECT]);
}

#[test]
fn custom_compiler_pattern_overlapping_copy_role_rejects_before_prefix_touch() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("prefix-input"), "must not be copied\n").unwrap();
    fs::write(
        p.join("commands.txt"),
        "touch prefix-written\ncp prefix-input copy-written\n",
    )
    .unwrap();
    let output = cli(p)
        .args([
            "--commands",
            "commands.txt",
            "--compiler-regex",
            r"^cp(?:\s|$)",
        ])
        .output()
        .unwrap();
    assert_rejected_without_effects(p, &output, &["prefix-written", "copy-written"]);
}

#[test]
fn invalid_compiler_regex_prevents_both_logged_commands_and_make_planning() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("prefix-input"), "must not be copied\n").unwrap();
    fs::write(p.join("commands.txt"), "cp prefix-input prefix-written\n").unwrap();
    let output = cli(p)
        .args(["--commands", "commands.txt", "--compiler-regex", "["])
        .output()
        .unwrap();
    assert_rejected_without_effects(p, &output, &["prefix-written"]);

    if Command::new("make").arg("--version").output().is_err() {
        eprintln!("skipping Make expansion fixture: make is not installed");
        return;
    }
    fs::write(
        p.join("Makefile"),
        "$(file >make-planned,planning must not happen)\nall:\n\tcp prefix-input prefix-written\n",
    )
    .unwrap();
    let output = cli(p).args(["--compiler-regex", "["]).output().unwrap();
    assert_rejected_without_effects(p, &output, &["make-planned", "prefix-written"]);
}
