use rusty_make::{
    commands::{ar::Ar, nm::Nm, objcopy::Objcopy},
    graph::create_graph,
    job::{Access, FileAccess},
    parse,
    registry::Registry,
    scheduler::execute,
};
use std::{any::Any, fs, process::Command};

fn available(tools: &[&str]) -> bool {
    tools.iter().all(|tool| {
        let found = Command::new(tool).arg("--version").output().is_ok();
        if !found {
            eprintln!("skipping unavailable tool {tool}");
        }
        found
    })
}

#[test]
fn registers_exact_gnu_and_llvm_names_without_regex_matching() {
    let registry = Registry::default();
    for executable in [
        "nm",
        "gcc-nm",
        "llvm-nm",
        "objdump",
        "llvm-objdump",
        "readelf",
        "llvm-readelf",
        "size",
        "llvm-size",
        "strings",
        "llvm-strings",
        "addr2line",
        "llvm-addr2line",
        "c++filt",
        "llvm-cxxfilt",
        "objcopy",
        "llvm-objcopy",
        "strip",
        "llvm-strip",
        "ar",
        "gcc-ar",
        "llvm-ar",
        "ranlib",
        "gcc-ranlib",
        "llvm-ranlib",
        "as",
        "llvm-as",
        "ld",
        "ld.bfd",
        "ld.gold",
        "ld.lld",
        "lld-link",
        "elfedit",
        "dwp",
        "llvm-dwp",
        "readobj",
        "llvm-readobj",
        "dwarfdump",
        "llvm-dwarfdump",
        "symbolizer",
        "llvm-symbolizer",
    ] {
        let jobs = parse(
            &format!("{executable} --future-option input.o > report"),
            &registry,
        )
        .unwrap();
        assert!(jobs[0].data().unknown_effects, "{executable}");
        assert!(
            jobs[0]
                .data()
                .accesses
                .contains(&FileAccess::write("report"))
        );
    }
    let jobs = parse("/usr/bin/nm input.o", &registry).unwrap();
    let job = (jobs[0].as_ref() as &dyn Any).downcast_ref::<Nm>().unwrap();
    assert_eq!(job.argv, ["/usr/bin/nm", "input.o"]);
    for executable in ["nm-wrapper", "arm-none-eabi-nm", "llvm-nm-18"] {
        assert!(parse(&format!("{executable} input.o"), &registry).is_err());
    }
    let mut custom = Registry::empty();
    custom
        .register(&["arm-none-eabi-nm"], |argv, hints| {
            Ok(Some(Box::new(Nm::parse(argv, hints)?)))
        })
        .unwrap();
    let jobs = parse("arm-none-eabi-nm input.o", &custom).unwrap();
    let job = (jobs[0].as_ref() as &dyn Any).downcast_ref::<Nm>().unwrap();
    assert_eq!(job.argv, ["arm-none-eabi-nm", "input.o"]);
}

#[test]
fn supported_arguments_describe_real_reads_and_writes() {
    let registry = Registry::default();
    for (command, expected) in [
        ("nm -C input.o", vec![FileAccess::read("input.o")]),
        ("nm", vec![FileAccess::read("a.out")]),
        ("objdump -h input.o", vec![FileAccess::read("input.o")]),
        ("readelf -h input.o", vec![FileAccess::read("input.o")]),
        ("size -A input.o", vec![FileAccess::read("input.o")]),
        ("strings -n 8 input.o", vec![FileAccess::read("input.o")]),
        ("c++filt _Z3foov", vec![]),
        (
            "objcopy input.o output.o",
            vec![FileAccess::read("input.o"), FileAccess::write("output.o")],
        ),
        ("objcopy input.o", vec![FileAccess::read_write("input.o")]),
        (
            "strip --strip-debug input.o",
            vec![FileAccess::read_write("input.o")],
        ),
        (
            "strip -o output.o input.o",
            vec![FileAccess::read("input.o"), FileAccess::write("output.o")],
        ),
        (
            "ranlib library.a",
            vec![FileAccess::read_write("library.a")],
        ),
        (
            "elfedit --output-osabi=GNU input.o",
            vec![FileAccess::read_write("input.o")],
        ),
        (
            "dwp -o package.dwp one.dwo",
            vec![
                FileAccess::read("one.dwo"),
                FileAccess::write("package.dwp"),
            ],
        ),
        (
            "llvm-readobj --file-headers input.o",
            vec![FileAccess::read("input.o")],
        ),
    ] {
        let jobs = parse(command, &registry).unwrap();
        assert!(!jobs[0].data().unknown_effects, "{command}");
        assert_eq!(jobs[0].data().accesses, expected, "{command}");
    }
    let jobs = parse("ar rcs library.a one.o two.o", &registry).unwrap();
    assert_eq!(
        jobs[0].data().accesses,
        [
            FileAccess::read_write("library.a"),
            FileAccess::read("one.o"),
            FileAccess::read("two.o")
        ]
    );
    assert!((jobs[0].as_ref() as &dyn Any).is::<Ar>());
}

#[test]
fn hidden_inputs_remain_unknown_and_original_argv_is_retained() {
    let registry = Registry::default();
    for command in [
        "nm --plugin=plugin.so input.o",
        "nm @options",
        "nm -- @options",
        "objdump -S input.o",
        "objdump -d input.o",
        "readelf --process-links input.o",
        "addr2line -e input.o 0",
        "as -o output.o input.s",
        "ld -r -o output.o input.o",
        "ar rcT library.a input.o",
        "ar x library.a",
        "llvm-symbolizer --obj=input.o 0",
        "llvm-dwarfdump input.o",
        "dwp -e app -o out.dwp",
        "strings",
    ] {
        assert!(
            parse(command, &registry).unwrap()[0].data().unknown_effects,
            "{command}"
        );
    }
    let jobs = parse(
        "objcopy --future-option 'literal value' input.o > output",
        &registry,
    )
    .unwrap();
    let job = (jobs[0].as_ref() as &dyn Any)
        .downcast_ref::<Objcopy>()
        .unwrap();
    assert_eq!(
        job.argv,
        ["objcopy", "--future-option", "literal value", "input.o"]
    );
    assert_eq!(job.data.shell.stdout.as_deref(), Some("output"));
}

#[test]
fn thin_archive_discovery_keeps_explicit_inputs_but_marks_hidden_members() {
    let root = tempfile::tempdir().unwrap();
    let archive = format!("{}/thin.a", root.path().display());
    fs::write(&archive, b"!<thin>\n").unwrap();
    let jobs = parse(&format!("nm '{archive}'"), &Registry::default()).unwrap();
    assert!(jobs[0].data().unknown_effects);
    assert_eq!(jobs[0].data().accesses, [FileAccess::read(archive)]);
}

#[test]
fn installed_inspection_tools_execute_and_redirect_their_exact_output() {
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
    let source = format!("{}/value.c", root.path().display());
    let object = format!("{}/input.o", root.path().display());
    fs::write(
        &source,
        "const char *marker = \"rusty_binary_marker\";\nint exported_value(void) { return 7; }\n",
    )
    .unwrap();
    assert!(
        Command::new("cc")
            .args(["-g", "-c", &source, "-o", &object])
            .status()
            .unwrap()
            .success()
    );
    for (tool, args) in [
        ("nm", vec!["--defined-only", object.as_str()]),
        ("objdump", vec!["-h", object.as_str()]),
        ("readelf", vec!["-h", object.as_str()]),
        ("size", vec![object.as_str()]),
        ("strings", vec![object.as_str()]),
        ("addr2line", vec!["-e", object.as_str(), "0"]),
        ("c++filt", vec!["_Z3foov"]),
    ] {
        let reference = Command::new(tool).args(&args).output().unwrap();
        assert!(reference.status.success());
        let output = format!("{}/{tool}.txt", root.path().display());
        fs::write(&output, "stale content").unwrap();
        let command = format!(
            "{tool} {} > '{output}'",
            args.iter()
                .map(|arg| format!("'{arg}'"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        let dag = create_graph(parse(&command, &Registry::default()).unwrap()).unwrap();
        dag.check_inputs().unwrap();
        execute(&dag, false).unwrap();
        assert_eq!(fs::read(output).unwrap(), reference.stdout, "{tool}");
    }
}

#[test]
fn installed_archive_and_mutating_tools_preserve_real_file_effects() {
    if !available(&["cc", "ar", "ranlib", "nm", "objcopy", "strip", "readelf"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let p = root.path().display();
    fs::write(
        format!("{p}/value.c"),
        "int exported_value(void) { return 7; }\n",
    )
    .unwrap();
    assert!(
        Command::new("cc")
            .args([
                "-g",
                "-c",
                &format!("{p}/value.c"),
                "-o",
                &format!("{p}/input.o")
            ])
            .status()
            .unwrap()
            .success()
    );
    let text = format!(
        "ar rcs '{p}/library.a' '{p}/input.o'\nranlib '{p}/library.a'\nnm '{p}/library.a' > '{p}/symbols'\nobjcopy --only-section=.text '{p}/input.o' '{p}/code.o'\nstrip --strip-debug '{p}/code.o'\nreadelf -S '{p}/code.o' > '{p}/sections'"
    );
    let dag = create_graph(parse(&text, &Registry::default()).unwrap()).unwrap();
    dag.check_inputs().unwrap();
    execute(&dag, false).unwrap();
    assert!(
        fs::read_to_string(format!("{p}/symbols"))
            .unwrap()
            .contains("exported_value")
    );
    assert!(
        fs::read_to_string(format!("{p}/sections"))
            .unwrap()
            .contains(".text")
    );
    assert!(
        dag.jobs
            .iter()
            .flat_map(|job| &job.data().accesses)
            .any(|access| access.access == Access::ReadWrite)
    );
}

#[test]
fn installed_llvm_aliases_execute_their_original_invocations() {
    if !available(&["cc", "llvm-ar", "llvm-nm", "llvm-objcopy", "llvm-readobj"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let p = root.path().display();
    fs::write(
        format!("{p}/input.c"),
        "int llvm_export(void) { return 9; }\n",
    )
    .unwrap();
    assert!(
        Command::new("cc")
            .args(["-c", &format!("{p}/input.c"), "-o", &format!("{p}/input.o")])
            .status()
            .unwrap()
            .success()
    );
    let text = format!(
        "llvm-ar rcs '{p}/library.a' '{p}/input.o'\nllvm-nm --defined-only '{p}/library.a' > '{p}/symbols'\nllvm-objcopy '{p}/input.o' '{p}/copy.o'\nllvm-readobj --file-headers '{p}/copy.o' > '{p}/header'"
    );
    let dag = create_graph(parse(&text, &Registry::default()).unwrap()).unwrap();
    execute(&dag, false).unwrap();
    assert!(
        fs::read_to_string(format!("{p}/symbols"))
            .unwrap()
            .contains("llvm_export")
    );
    assert!(
        fs::read_to_string(format!("{p}/header"))
            .unwrap()
            .contains("Format:")
    );
}
