use rusty_make::{
    parse_plan,
    registry::Registry,
    scheduler::{RunOptions, run},
};
use std::{fs, path::Path, process::Command};

fn tools(names: &[&str]) -> bool {
    names.iter().all(|name| {
        let found = Command::new(name).arg("--version").output().is_ok();
        if !found {
            eprintln!("skipping unavailable tool {name}");
        }
        found
    })
}
fn object(root: &Path, name: &str, symbol: &str) {
    fs::write(
        root.join(format!("{name}.c")),
        format!("int {symbol}(void) {{ return 7; }}\n"),
    )
    .unwrap();
    let result = Command::new("cc")
        .current_dir(root)
        .args(["-g", "-c", &format!("{name}.c"), "-o", &format!("{name}.o")])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
fn direct(root: &Path, program: &str, args: &[&str]) -> Vec<u8> {
    let result = Command::new(program)
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}

#[test]
fn independent_inspection_commands_share_a_wave_and_execute() {
    if !tools(&["cc", "nm", "objdump", "readelf", "strings"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    object(p, "a", "alpha");
    object(p, "b", "beta");
    let graph=parse_plan("nm --defined-only a.o > a.nm\nobjdump -h b.o > b.sections\nreadelf -h a.o > a.header\nstrings b.o > b.strings\n",p,&Registry::default()).unwrap();
    assert_eq!(graph.waves, vec![vec![0, 1, 2, 3]]);
    assert!(graph.edges.is_empty());
    assert!(graph.jobs.iter().all(|j| !j.spec.unknown_effects));
    let report = run(
        &graph,
        &RunOptions {
            jobs: 4,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(report.executed, 4);
    for (file, program, args) in [
        ("a.nm", "nm", vec!["--defined-only", "a.o"]),
        ("b.sections", "objdump", vec!["-h", "b.o"]),
        ("a.header", "readelf", vec!["-h", "a.o"]),
        ("b.strings", "strings", vec!["b.o"]),
    ] {
        assert_eq!(fs::read(p.join(file)).unwrap(), direct(p, program, &args));
    }
}

#[test]
fn writes_readers_and_inplace_mutations_keep_real_hazards() {
    if !tools(&["cc", "nm", "objcopy", "strip", "readelf"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    object(p, "source", "alpha");
    let graph=parse_plan("cp source.o build.o\nnm --defined-only build.o > symbols.txt\nobjcopy --only-section=.text build.o code.o\nstrip --strip-debug build.o\nreadelf -h build.o > header.txt\n",p,&Registry::default()).unwrap();
    assert_eq!(graph.waves, vec![vec![0], vec![1, 2], vec![3], vec![4]]);
    assert!(
        graph
            .edges
            .iter()
            .any(|e| e.from == 1 && e.to == 3 && e.reason == "write after read")
    );
    assert!(
        graph
            .edges
            .iter()
            .any(|e| e.from == 2 && e.to == 3 && e.reason == "write after read")
    );
    run(
        &graph,
        &RunOptions {
            jobs: 4,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        fs::read_to_string(p.join("symbols.txt"))
            .unwrap()
            .contains("alpha")
    );
    assert_eq!(
        fs::read(p.join("header.txt")).unwrap(),
        direct(p, "readelf", &["-h", "build.o"])
    );
    let sections = String::from_utf8(direct(p, "readelf", &["-S", "code.o"])).unwrap();
    assert!(sections.contains(".text"));
}

#[test]
fn section_payloads_and_multiple_outputs_are_tracked() {
    if !tools(&["cc", "objcopy"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    object(p, "input", "alpha");
    fs::write(p.join("payload.bin"), "old payload").unwrap();
    fs::write(p.join("replacement.bin"), "new payload").unwrap();
    let graph=parse_plan("objcopy --add-section=.payload=payload.bin --dump-section=.text=code.bin input.o transformed.o\ncp code.bin copied-code.bin\ncp replacement.bin payload.bin\n",p,&Registry::default()).unwrap();
    assert_eq!(graph.waves, vec![vec![0], vec![1, 2]]);
    assert!(
        graph
            .edges
            .iter()
            .any(|e| e.from == 0 && e.to == 2 && e.reason == "write after read")
    );
    run(
        &graph,
        &RunOptions {
            jobs: 3,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        fs::read(p.join("code.bin")).unwrap(),
        fs::read(p.join("copied-code.bin")).unwrap()
    );
    direct(
        p,
        "objcopy",
        &[
            "--dump-section=.payload=extracted.bin",
            "transformed.o",
            "verify.o",
        ],
    );
    assert_eq!(fs::read(p.join("extracted.bin")).unwrap(), b"old payload");
    assert_eq!(fs::read(p.join("payload.bin")).unwrap(), b"new payload");
}

#[test]
fn archive_work_orders_only_archive_consumers() {
    if !tools(&["cc", "ar", "ranlib", "nm"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    object(p, "a", "alpha");
    object(p, "b", "beta");
    let graph=parse_plan("ar rcs library.a a.o\nranlib library.a\nnm library.a > archive.txt\nnm b.o > independent.txt\n",p,&Registry::default()).unwrap();
    assert_eq!(graph.waves, vec![vec![0, 3], vec![1], vec![2]]);
    run(
        &graph,
        &RunOptions {
            jobs: 3,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        fs::read_to_string(p.join("archive.txt"))
            .unwrap()
            .contains("alpha")
    );
    assert!(
        fs::read_to_string(p.join("independent.txt"))
            .unwrap()
            .contains("beta")
    );
}

#[test]
fn append_dependencies_are_not_lost_when_inspection_is_precise() {
    if !tools(&["cc", "nm", "objdump"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    object(p, "a", "alpha");
    object(p, "b", "beta");
    let graph = parse_plan(
        "nm a.o > report.txt\nobjdump -h b.o >> report.txt\ncp a.c independent.c\n",
        p,
        &Registry::default(),
    )
    .unwrap();
    assert_eq!(graph.waves, vec![vec![0, 2], vec![1]]);
    assert_eq!(
        graph.reads[1]
            .iter()
            .find(|a| a.path.ends_with("report.txt"))
            .unwrap()
            .version,
        1
    );
    run(
        &graph,
        &RunOptions {
            jobs: 3,
            ..Default::default()
        },
    )
    .unwrap();
    let mut expected = direct(p, "nm", &["a.o"]);
    expected.extend(direct(p, "objdump", &["-h", "b.o"]));
    assert_eq!(fs::read(p.join("report.txt")).unwrap(), expected);
}

#[test]
fn unknown_flags_and_hidden_dependency_modes_still_parse_but_stay_opaque() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    for command in [
        "nm --plugin=custom.so a.o > a.nm\n",
        "objdump -S a.o > source.txt\n",
        "objdump -d a.o > disassembly.txt\n",
        "addr2line -e a.o 0 > addresses.txt\n",
        "as input.s -o input.o\n",
        "ld -T layout.ld input.o -o app\n",
        "nm @options.rsp > symbols\n",
    ] {
        let graph = parse_plan(command, p, &Registry::default()).unwrap();
        assert!(graph.jobs[0].spec.unknown_effects, "{command}");
        assert!(
            graph.jobs[0].spec.parsed["effect_reason"].is_string(),
            "{command}"
        );
    }
    let graph = parse_plan(
        "objdump -d --dwarf=no-follow-links a.o > disassembly.txt\n",
        p,
        &Registry::default(),
    )
    .unwrap();
    assert!(!graph.jobs[0].spec.unknown_effects);
}

#[test]
fn existing_thin_archives_keep_their_external_member_barrier() {
    if !tools(&["cc", "ar", "nm"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    object(p, "a", "alpha");
    direct(p, "ar", &["rcT", "thin.a", "a.o"]);
    let graph = parse_plan(
        "nm thin.a > symbols.txt\ncp a.c later.c\n",
        p,
        &Registry::default(),
    )
    .unwrap();
    assert!(graph.jobs[0].spec.unknown_effects);
    assert!(
        graph.jobs[0].spec.parsed["effect_reason"]
            .as_str()
            .unwrap()
            .contains("thin archive")
    );
    assert_eq!(graph.waves, vec![vec![0], vec![1]]);
    run(&graph, &RunOptions::default()).unwrap();
    assert!(
        fs::read_to_string(p.join("symbols.txt"))
            .unwrap()
            .contains("alpha")
    );
}

#[test]
fn unchanged_inspection_output_does_not_invalidate_a_native_consumer() {
    if !tools(&["cc", "nm"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    object(p, "a", "alpha");
    let graph = parse_plan(
        "nm a.o > symbols.txt\ncp symbols.txt final.txt\n",
        p,
        &Registry::default(),
    )
    .unwrap();
    let first = run(&graph, &RunOptions::default()).unwrap();
    assert_eq!((first.executed, first.skipped), (2, 0));
    let second = run(&graph, &RunOptions::default()).unwrap();
    assert_eq!((second.executed, second.skipped), (1, 1));
    assert_eq!(
        fs::read(p.join("final.txt")).unwrap(),
        direct(p, "nm", &["a.o"])
    );
}

#[test]
fn newly_created_thin_archive_cannot_silently_invalidate_a_precise_plan() {
    if !tools(&["cc", "ar", "nm"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    object(p, "a", "alpha");
    let text = "ar rcT thin.a a.o\nnm thin.a > symbols.txt\n";
    let graph = parse_plan(text, p, &Registry::default()).unwrap();
    let error = run(&graph, &RunOptions::default()).unwrap_err();
    assert!(format!("{error:#}").contains("thin archive"));
    assert!(!p.join("symbols.txt").exists());
    // Replanning can now discover the external-member archive and use a barrier.
    let graph = parse_plan(text, p, &Registry::default()).unwrap();
    assert!(graph.jobs[1].spec.unknown_effects);
    run(&graph, &RunOptions::default()).unwrap();
    assert!(
        fs::read_to_string(p.join("symbols.txt"))
            .unwrap()
            .contains("alpha")
    );
}
