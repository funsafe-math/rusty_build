use rusty_make::{
    graph::create_graph, job::FileAccess, parse, registry::Registry, scheduler::execute,
};
use std::{fs, process::Command};

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
fn independent_inspections_share_a_wave_without_unknown_barriers() {
    let dag = create_graph(parse(
        "nm --defined-only a.o > a.nm\nobjdump -h b.o > b.sections\nreadelf -h a.o > a.header\nstrings b.o > b.strings",
        &Registry::default(),
    ).unwrap()).unwrap();
    assert_eq!(dag.waves, [vec![0, 1, 2, 3]]);
    assert!(dag.edges.is_empty());
    assert!(dag.jobs.iter().all(|job| !job.data().unknown_effects));
}

#[test]
fn reads_precede_overwrites_and_archive_consumers_follow_mutations() {
    let dag = create_graph(parse(
        "cp source.o build.o\nnm build.o > symbols\nobjcopy build.o code.o\nstrip --strip-debug build.o\nreadelf -h build.o > header",
        &Registry::default(),
    ).unwrap()).unwrap();
    assert_eq!(dag.waves, [vec![0], vec![1, 2], vec![3], vec![4]]);
    for reader in [1, 2] {
        assert!(
            dag.edges
                .iter()
                .any(|edge| edge.from == reader && edge.to == 3)
        );
    }
    let dag = create_graph(parse(
        "ar rcs library.a a.o\nranlib library.a\nnm library.a > archive.txt\nnm b.o > independent.txt",
        &Registry::default(),
    ).unwrap()).unwrap();
    assert_eq!(dag.waves, [vec![0, 3], vec![1], vec![2]]);
}

#[test]
fn section_payload_reads_and_secondary_outputs_form_real_dependencies() {
    let dag = create_graph(parse(
        "objcopy --add-section=.payload=payload.bin --dump-section=.text=code.bin input.o transformed.o\ncp code.bin copied-code.bin\ncp replacement.bin payload.bin",
        &Registry::default(),
    ).unwrap()).unwrap();
    assert_eq!(dag.waves, [vec![0], vec![1, 2]]);
    assert_eq!(
        dag.jobs[0].data().accesses,
        [
            FileAccess::read("input.o"),
            FileAccess::read("payload.bin"),
            FileAccess::write("transformed.o"),
            FileAccess::write("code.bin"),
        ]
    );
}

#[test]
fn simple_sed_substitution_and_backups_have_precise_file_accesses() {
    let registry = Registry::default();
    let jobs = parse(
        "sed 's/old/new/g' input > output\nsed -i.bak -e 's/new/final/' mutable",
        &registry,
    )
    .unwrap();
    assert!(!jobs[0].data().unknown_effects);
    assert_eq!(
        jobs[0].data().accesses,
        [FileAccess::read("input"), FileAccess::write("output")]
    );
    assert!(!jobs[1].data().unknown_effects);
    assert_eq!(
        jobs[1].data().accesses,
        [
            FileAccess::read_write("mutable"),
            FileAccess::write("mutable.bak")
        ]
    );
    let jobs = parse("sed --in-place='backup-*' s/a/b/ mutable", &registry).unwrap();
    assert_eq!(
        jobs[0].data().accesses,
        [
            FileAccess::read_write("mutable"),
            FileAccess::write("backup-mutable")
        ]
    );
    for command in [
        "sed 's/a/b/w hidden' input",
        "sed 's/a/b/e' input",
        "sed 'r hidden' input",
        "sed -f script.sed input",
        "sed 's/a/b/;s/c/d/' input",
    ] {
        assert!(
            parse(command, &registry).unwrap()[0].data().unknown_effects,
            "{command}"
        );
    }
}

#[test]
fn installed_sed_executes_substitution_inplace_and_preserves_backups() {
    if !available(&["sed"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let p = root.path().display();
    fs::write(format!("{p}/input"), "old value\n").unwrap();
    let text = format!(
        "sed 's/old/new/' '{p}/input' > '{p}/output'\nsed -i.bak 's/new/final/' '{p}/output'\ncp '{p}/output' '{p}/copy'"
    );
    let dag = create_graph(parse(&text, &Registry::default()).unwrap()).unwrap();
    assert_eq!(dag.waves, [vec![0], vec![1], vec![2]]);
    dag.check_inputs().unwrap();
    execute(&dag, false).unwrap();
    assert_eq!(fs::read(format!("{p}/input")).unwrap(), b"old value\n");
    assert_eq!(fs::read(format!("{p}/output.bak")).unwrap(), b"new value\n");
    assert_eq!(fs::read(format!("{p}/copy")).unwrap(), b"final value\n");
}

#[test]
fn installed_objcopy_executes_section_inputs_and_secondary_outputs() {
    if !available(&["cc", "objcopy"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let p = root.path().display();
    fs::write(format!("{p}/input.c"), "int alpha(void) { return 7; }\n").unwrap();
    fs::write(format!("{p}/payload"), b"original payload").unwrap();
    assert!(
        Command::new("cc")
            .args(["-c", &format!("{p}/input.c"), "-o", &format!("{p}/input.o")])
            .status()
            .unwrap()
            .success()
    );
    let text = format!(
        "objcopy '--add-section=.payload={p}/payload' '--dump-section=.text={p}/code' '{p}/input.o' '{p}/transformed.o'\ncp '{p}/code' '{p}/code-copy'"
    );
    let dag = create_graph(parse(&text, &Registry::default()).unwrap()).unwrap();
    dag.check_inputs().unwrap();
    execute(&dag, false).unwrap();
    assert_eq!(
        fs::read(format!("{p}/code")).unwrap(),
        fs::read(format!("{p}/code-copy")).unwrap()
    );
    assert!(
        Command::new("objcopy")
            .args([
                &format!("--dump-section=.payload={p}/extracted"),
                &format!("{p}/transformed.o"),
                &format!("{p}/verify.o"),
            ])
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        fs::read(format!("{p}/extracted")).unwrap(),
        b"original payload"
    );
}

#[test]
fn tool_failure_redirects_diagnostics_and_stops_dependents() {
    if !available(&["objcopy"]) {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let p = root.path().display();
    fs::write(format!("{p}/invalid.o"), b"not an object file").unwrap();
    let text = format!(
        "objcopy '{p}/invalid.o' '{p}/result.o' 2> '{p}/errors'\ncp '{p}/result.o' '{p}/dependent.o'"
    );
    let dag = create_graph(parse(&text, &Registry::default()).unwrap()).unwrap();
    assert!(execute(&dag, false).is_err());
    assert!(!fs::read(format!("{p}/errors")).unwrap().is_empty());
    assert!(!root.path().join("dependent.o").exists());
}
