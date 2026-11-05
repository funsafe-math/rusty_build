use rusty_make::{
    input::SourceCommand,
    job::ExecutionStrategy,
    parse_plan,
    registry::Registry,
    scheduler::{RunOptions, run},
};
use std::{fs, path::Path, process::Command};

fn source(text: &str) -> SourceCommand {
    SourceCommand {
        line: 1,
        text: text.into(),
        target: None,
    }
}
fn available(tools: &[&str]) -> bool {
    for tool in tools {
        if Command::new(tool).arg("--version").output().is_err() {
            eprintln!("skipping unavailable binary tool: {tool}");
            return false;
        }
    }
    true
}
fn object(root: &Path) {
    fs::write(root.join("value.c"), "const char *marker = \"rusty_binary_tool_marker\";\nint exported_value(void) { return 7; }\n").unwrap();
    let result = Command::new("cc")
        .current_dir(root)
        .args(["-g", "-c", "value.c", "-o", "input.o"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn identifies_gnu_llvm_cross_and_absolute_tool_names_without_flag_parsing() {
    let registry = Registry::default();
    for tool in [
        "nm",
        "objdump",
        "readelf",
        "size",
        "strings",
        "addr2line",
        "c++filt",
        "objcopy",
        "strip",
        "ar",
        "ranlib",
        "as",
        "ld",
        "elfedit",
        "dwp",
        "readobj",
        "dwarfdump",
        "symbolizer",
    ] {
        for program in [
            tool.to_owned(),
            format!("/usr/bin/{tool}"),
            format!("arm-none-eabi-{tool}"),
            format!("llvm-{tool}-18"),
        ] {
            let text = format!("{program} --future-option=value input.o > 'tool report.txt'");
            let job = registry.classify(source(&text)).unwrap();
            assert_eq!(job.spec.kind, tool, "{text}");
            assert_eq!(job.strategy, ExecutionStrategy::External);
            assert!(job.spec.unknown_effects && !job.spec.cacheable);
            assert_eq!(
                job.shell.as_ref().unwrap().args,
                vec!["--future-option=value", "input.o"]
            );
            assert_eq!(
                job.spec.outputs,
                vec![std::path::PathBuf::from("tool report.txt")]
            );
        }
    }
    for (text, kind) in [
        ("'tools with spaces/llvm-objdump-18' -h input.o", "objdump"),
        ("llvm-cxxfilt _Z3foov", "c++filt"),
        ("x86_64-linux-gnu-ld.bfd input.o", "ld"),
        ("ld.lld input.o", "ld"),
        ("aarch64-none-elf-nm.exe input.o", "nm"),
    ] {
        assert_eq!(registry.classify(source(text)).unwrap().spec.kind, kind);
    }
    for text in [
        "nm-wrapper input.o",
        "objdump_extra input.o",
        "notnm input.o",
    ] {
        assert_eq!(
            registry.classify(source(text)).unwrap().spec.kind,
            "unknown"
        );
    }
}

#[test]
fn file_redirections_keep_declared_effects_and_opaque_descriptor_ordering() {
    let registry = Registry::default();
    let append = registry
        .classify(source("nm input.o >> symbols.txt"))
        .unwrap();
    assert_eq!(
        append.spec.inputs,
        vec![
            std::path::PathBuf::from("input.o"),
            std::path::PathBuf::from("symbols.txt")
        ]
    );
    assert_eq!(
        append.spec.outputs,
        vec![std::path::PathBuf::from("symbols.txt")]
    );
    assert!(append.shell.unwrap().append_stdout);
    let stderr = registry
        .classify(source("objdump --bad 2> errors.txt"))
        .unwrap();
    assert_eq!(
        stderr.spec.outputs,
        vec![std::path::PathBuf::from("errors.txt")]
    );
    let stdin = registry.classify(source("c++filt < names.txt")).unwrap();
    assert_eq!(
        stdin.spec.inputs,
        vec![std::path::PathBuf::from("names.txt")]
    );
    let opaque = registry
        .classify(source("nm input.o > both.txt 2>&1"))
        .unwrap();
    assert_eq!(opaque.spec.kind, "nm");
    assert!(opaque.shell.is_none());
    assert!(opaque.spec.unknown_effects);
}

#[test]
fn real_inspection_tools_redirect_and_append_exactly_their_output() {
    if !available(&[
        "cc",
        "nm",
        "objdump",
        "readelf",
        "size",
        "strings",
        "addr2line",
        "c++filt",
    ]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    object(p);
    for (tool, args, file) in [
        ("nm", vec!["--defined-only", "input.o"], "symbols.txt"),
        ("objdump", vec!["-h", "input.o"], "sections.txt"),
        ("readelf", vec!["-h", "input.o"], "headers.txt"),
        ("size", vec!["input.o"], "size.txt"),
        ("strings", vec!["input.o"], "strings.txt"),
        ("addr2line", vec!["-e", "input.o", "0"], "lines.txt"),
    ] {
        let reference = Command::new(tool)
            .args(&args)
            .current_dir(p)
            .output()
            .unwrap();
        assert!(reference.status.success());
        fs::write(p.join(file), "stale content that must be truncated").unwrap();
        let text = format!("{tool} {} > {file}\n", args.join(" "));
        let graph = parse_plan(&text, p, &Registry::default()).unwrap();
        run(
            &graph,
            &RunOptions {
                jobs: 4,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(fs::read(p.join(file)).unwrap(), reference.stdout, "{tool}");
    }
    let reference = Command::new("nm")
        .args(["--defined-only", "input.o"])
        .current_dir(p)
        .output()
        .unwrap()
        .stdout;
    let graph = parse_plan(
        "nm --defined-only input.o >> symbols.txt\n",
        p,
        &Registry::default(),
    )
    .unwrap();
    run(&graph, &RunOptions::default()).unwrap();
    assert_eq!(
        fs::read(p.join("symbols.txt")).unwrap(),
        [reference.as_slice(), reference.as_slice()].concat()
    );
    fs::write(p.join("mangled names.txt"), "_Z3foov\n").unwrap();
    let graph = parse_plan(
        "c++filt < 'mangled names.txt' > 'demangled names.txt'\n",
        p,
        &Registry::default(),
    )
    .unwrap();
    run(&graph, &RunOptions::default()).unwrap();
    assert_eq!(
        fs::read_to_string(p.join("demangled names.txt")).unwrap(),
        "foo()\n"
    );
}

#[test]
fn real_mutating_tools_preserve_order_and_allow_file_redirection() {
    if !available(&["cc", "ar", "ranlib", "nm", "objcopy", "strip", "objdump"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    object(p);
    let text = "ar rcs library.a input.o > archive.log\nranlib library.a > ranlib.log\nnm library.a > archive-symbols.txt\nobjcopy --only-section=.text input.o code.o > objcopy.log\nstrip --strip-debug code.o > strip.log\nobjdump -h code.o > code-sections.txt\n";
    let graph = parse_plan(text, p, &Registry::default()).unwrap();
    let report = run(
        &graph,
        &RunOptions {
            jobs: 4,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(report.executed, 6);
    assert!(
        fs::read_to_string(p.join("archive-symbols.txt"))
            .unwrap()
            .contains("exported_value")
    );
    assert!(
        fs::read_to_string(p.join("code-sections.txt"))
            .unwrap()
            .contains(".text")
    );
    for file in ["archive.log", "ranlib.log", "objcopy.log", "strip.log"] {
        assert_eq!(fs::read(p.join(file)).unwrap(), b"");
    }
}

#[test]
fn tool_errors_redirect_stderr_and_do_not_start_dependents() {
    if !available(&["nm"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    let reference = Command::new("nm")
        .args(["--rusty-make-invalid-option"])
        .current_dir(p)
        .output()
        .unwrap();
    assert!(!reference.status.success());
    let graph = parse_plan(
        "nm --rusty-make-invalid-option > stdout.txt 2> stderr.txt\ntouch never-started\n",
        p,
        &Registry::default(),
    )
    .unwrap();
    assert!(run(&graph, &RunOptions::default()).is_err());
    assert_eq!(fs::read(p.join("stdout.txt")).unwrap(), reference.stdout);
    assert_eq!(fs::read(p.join("stderr.txt")).unwrap(), reference.stderr);
    assert!(!p.join("never-started").exists());
    let graph = parse_plan(
        "nm --rusty-make-invalid-option > combined.txt 2>&1\n",
        p,
        &Registry::default(),
    )
    .unwrap();
    assert!(run(&graph, &RunOptions::default()).is_err());
    assert_eq!(
        fs::read(p.join("combined.txt")).unwrap(),
        [reference.stdout.as_slice(), reference.stderr.as_slice()].concat()
    );
    let graph = parse_plan(
        "nm --rusty-make-invalid-option 2>> stderr.txt\n",
        p,
        &Registry::default(),
    )
    .unwrap();
    assert!(run(&graph, &RunOptions::default()).is_err());
    assert_eq!(
        fs::read(p.join("stderr.txt")).unwrap(),
        [reference.stderr.as_slice(), reference.stderr.as_slice()].concat()
    );
}
