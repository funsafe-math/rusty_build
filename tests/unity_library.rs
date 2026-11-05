#[path = "../examples/support/unity_transform.rs"]
mod unity_transform;

use rusty_make::{
    commands::compiler::Compiler,
    graph::{BuildGraph, RequiredDependency},
    parse_plan,
    registry::Registry,
    scheduler::{RunOptions, run},
};
use std::{fs, path::Path, process::Command};
use unity_transform::{UnityArchive, UnitySource, apply_unity};

const COMMANDS: &str = include_str!("../fixtures/unity/library-commands.log");
const COMPILES: &str = "cc -O2 -c src/calc_add.c -o build/calc_add.o\n\
cc -O2 -c src/calc_mul.c -o build/calc_mul.o\n\
cc -O2 -c src/main.c -o build/main.o\n";
const EXPECTED: &[u8] = b"unity demo: 42 42\n";

fn available(tools: &[&str]) -> bool {
    for tool in tools {
        if Command::new(tool).arg("--version").output().is_err() {
            eprintln!("skipping unavailable binary tool: {tool}");
            return false;
        }
    }
    true
}

fn workspace() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("src")).unwrap();
    fs::create_dir(root.path().join("build")).unwrap();
    for (name, source) in [
        (
            "calc_add.c",
            include_str!("../fixtures/unity/src/calc_add.c"),
        ),
        (
            "calc_mul.c",
            include_str!("../fixtures/unity/src/calc_mul.c"),
        ),
        ("main.c", include_str!("../fixtures/unity/src/main.c")),
    ] {
        fs::write(root.path().join("src").join(name), source).unwrap();
    }
    root
}

fn graph(commands: &str, root: &Path) -> BuildGraph {
    parse_plan(commands, root, &Registry::default()).unwrap()
}

fn snapshot(graph: &BuildGraph) -> Vec<u8> {
    serde_json::to_vec(graph).unwrap()
}

fn tool(root: &Path, program: &str, args: &[&str]) -> Vec<u8> {
    let output = Command::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{program} {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn execute(graph: &BuildGraph) -> Vec<u8> {
    run(
        graph,
        &RunOptions {
            jobs: 4,
            rebuild_all: true,
            ..Default::default()
        },
    )
    .unwrap();
    let output = Command::new(graph.cwd.join("build/unity-app"))
        .current_dir(&graph.cwd)
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    assert!(output.stderr.is_empty(), "{:?}", output);
    output.stdout
}

fn members(root: &Path) -> Vec<String> {
    let listing = tool(root, "ar", &["t", "build/libcalc.a"]);
    String::from_utf8(listing)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn assert_unity_members(root: &Path, unrelated: bool) {
    let mut actual = members(root);
    actual.sort();
    let mut expected = vec!["tu-0.o".to_owned()];
    if unrelated {
        expected.push("unrelated.o".to_owned());
    }
    expected.sort();
    assert_eq!(actual, expected);
    assert!(!root.join("build/calc_add.o").exists());
    assert!(!root.join("build/calc_mul.o").exists());
    assert!(root.join(".rusty_make/unity/tu-0.o").is_file());
}

fn compiler_count(graph: &BuildGraph) -> usize {
    graph
        .jobs
        .iter()
        .filter(|job| job.typed::<Compiler>().is_some())
        .count()
}

fn unchanged_without_unity(commands: &str) {
    let root = workspace();
    let mut graph = graph(commands, root.path());
    let before = snapshot(&graph);
    assert_eq!(apply_unity(&mut graph).unwrap().groups, 0);
    assert_eq!(snapshot(&graph), before, "mutated graph for {commands}");
    assert!(!root.path().join(".rusty_make").exists());
    assert_eq!(fs::read_dir(root.path().join("build")).unwrap().count(), 0);
}

#[test]
fn static_library_matches_standalone_and_reexecutes_without_duplicate_members() {
    if !available(&["cc", "ar", "ranlib"]) {
        return;
    }
    let standalone = workspace();
    let unity = workspace();
    let original = graph(COMMANDS, standalone.path());
    let mut transformed = graph(COMMANDS, unity.path());
    let stats = apply_unity(&mut transformed).unwrap();
    assert_eq!(
        (stats.groups, stats.original_compiles, stats.unity_compiles),
        (1, 2, 1)
    );
    assert_eq!(compiler_count(&original), 4);
    assert_eq!(compiler_count(&transformed), 3);
    assert!(!unity.path().join(".rusty_make").exists());
    assert_eq!(fs::read_dir(unity.path().join("build")).unwrap().count(), 0);
    assert_eq!(execute(&original), EXPECTED);
    let mut original_members = members(standalone.path());
    original_members.sort();
    assert_eq!(original_members, ["calc_add.o", "calc_mul.o"]);
    assert_eq!(execute(&transformed), EXPECTED);
    assert_unity_members(unity.path(), false);
    assert_eq!(execute(&transformed), EXPECTED);
    assert_unity_members(unity.path(), false);
}

#[test]
fn existing_archive_loses_only_superseded_original_members() {
    if !available(&["cc", "ar", "ranlib"]) {
        return;
    }
    let root = workspace();
    let commands = COMMANDS.replace("ar rcs ", "ar -rcsD ");
    let original = graph(&commands, root.path());
    assert_eq!(execute(&original), EXPECTED);
    fs::write(
        root.path().join("src/unrelated.c"),
        "int unrelated_archive_member(void) { return 123; }\n",
    )
    .unwrap();
    tool(
        root.path(),
        "cc",
        &["-c", "src/unrelated.c", "-o", "build/unrelated.o"],
    );
    tool(
        root.path(),
        "ar",
        &["rcs", "build/libcalc.a", "build/unrelated.o"],
    );
    assert_eq!(members(root.path()).len(), 3);
    for object in ["build/calc_add.o", "build/calc_mul.o", "build/unrelated.o"] {
        fs::remove_file(root.path().join(object)).unwrap();
    }
    let mut transformed = graph(&commands, root.path());
    apply_unity(&mut transformed).unwrap();
    assert_eq!(execute(&transformed), EXPECTED);
    assert_unity_members(root.path(), true);
    assert_eq!(execute(&transformed), EXPECTED);
    assert_unity_members(root.path(), true);
    assert!(!root.path().join("build/unrelated.o").exists());
}

#[test]
fn persisted_library_graph_runs_with_registered_internal_jobs() {
    if !available(&["cc", "ar", "ranlib"]) {
        return;
    }
    let root = workspace();
    let mut registry = Registry::default();
    registry.register_type::<UnitySource>().unwrap();
    registry.register_type::<UnityArchive>().unwrap();
    let mut transformed = parse_plan(COMMANDS, root.path(), &registry).unwrap();
    apply_unity(&mut transformed).unwrap();
    let saved = snapshot(&transformed);
    let restored = BuildGraph::from_json(&saved, &registry).unwrap();
    assert_eq!(snapshot(&restored), saved);
    assert_eq!(execute(&restored), EXPECTED);
    assert_unity_members(root.path(), false);
    assert_eq!(execute(&restored), EXPECTED);
    assert_unity_members(root.path(), false);
}

#[test]
fn shared_library_pic_compiles_and_downstream_executable_match_standalone() {
    if !available(&["cc"]) {
        return;
    }
    for pic in ["-fPIC", "-fpic"] {
        let commands = format!(
            "cc -O2 {pic} -c src/calc_add.c -o build/calc_add.o\n\
             cc -O2 {pic} -c src/calc_mul.c -o build/calc_mul.o\n\
             cc -O2 -c src/main.c -o build/main.o\n\
             cc -shared build/calc_add.o build/calc_mul.o -o build/libcalc.so\n\
             cc build/main.o build/libcalc.so -o build/unity-app\n"
        );
        let standalone = workspace();
        let unity = workspace();
        let original = graph(&commands, standalone.path());
        let mut transformed = graph(&commands, unity.path());
        let stats = apply_unity(&mut transformed).unwrap();
        assert_eq!(stats.unity_compiles, 1);
        assert_eq!(compiler_count(&original), 5);
        assert_eq!(compiler_count(&transformed), 4);
        assert_eq!(execute(&original), EXPECTED);
        assert_eq!(execute(&transformed), EXPECTED);
        assert!(unity.path().join("build/libcalc.so").is_file());
        assert!(!unity.path().join("build/calc_add.o").exists());
        assert!(!unity.path().join("build/calc_mul.o").exists());
    }
}

fn library_operand_workspace() -> tempfile::TempDir {
    let root = workspace();
    fs::write(
        root.path().join("src/calc_add.c"),
        "int library_delta(void);\nint calc_add(int a, int b) { return a + b + library_delta(); }\n",
    )
    .unwrap();
    for (name, definition) in [
        ("prefix", "int unused_prefix(void) { return 17; }\n"),
        ("extra", "int library_delta(void) { return 0; }\n"),
    ] {
        let source = format!("src/{name}.c");
        let object = format!("build/{name}.o");
        let archive = format!("build/lib{name}.a");
        fs::write(root.path().join(&source), definition).unwrap();
        tool(root.path(), "cc", &["-c", &source, "-o", &object]);
        tool(root.path(), "ar", &["rcs", &archive, &object]);
    }
    root
}

#[test]
fn direct_object_group_preserves_explicit_archive_operand_order() {
    if !available(&["cc", "ar"]) {
        return;
    }
    let standalone = library_operand_workspace();
    let unity = library_operand_workspace();
    let commands = format!(
        "{COMPILES}cc build/libprefix.a build/main.o build/calc_mul.o build/calc_add.o build/libextra.a -o build/unity-app\n"
    );
    let original = graph(&commands, standalone.path());
    let mut transformed = graph(&commands, unity.path());
    apply_unity(&mut transformed).unwrap();
    assert_eq!(execute(&original), EXPECTED);
    assert_eq!(execute(&transformed), EXPECTED);
    assert!(!unity.path().join("build/calc_add.o").exists());
    assert!(!unity.path().join("build/calc_mul.o").exists());
}

#[test]
fn object_groups_on_opposite_sides_of_link_boundaries_stay_standalone() {
    for boundary in ["build/libother.a", "build/libother.so", "-shared"] {
        unchanged_without_unity(&format!(
            "{COMPILES}cc build/main.o build/calc_add.o {boundary} build/calc_mul.o -o build/unity-app\n"
        ));
    }
}

#[test]
fn split_partial_duplicate_and_early_archive_consumers_stay_standalone() {
    for commands in [
        format!(
            "{COMPILES}ar rcs build/libone.a build/calc_add.o\n\
             ar rcs build/libtwo.a build/calc_mul.o\n\
             cc build/main.o build/libone.a build/libtwo.a -o build/unity-app\n"
        ),
        COMMANDS.replace("build/calc_add.o build/calc_mul.o\n", "build/calc_add.o\n"),
        COMMANDS.replace(
            "build/calc_add.o build/calc_mul.o\n",
            "build/calc_add.o build/calc_mul.o build/calc_add.o\n",
        ),
        format!("{COMMANDS}ar rcs build/partial.a build/calc_mul.o\n"),
        String::from(
            "cc -O2 -c src/calc_add.c -o build/calc_add.o\n\
             ar rcs build/libcalc.a build/calc_add.o build/calc_mul.o\n\
             cc -O2 -c src/calc_mul.c -o build/calc_mul.o\n\
             cc -O2 -c src/main.c -o build/main.o\n\
             cc build/main.o build/libcalc.a -o build/unity-app\n",
        ),
    ] {
        unchanged_without_unity(&commands);
    }
}

#[test]
fn opaque_archive_modes_leave_the_original_graph_unchanged() {
    for archive in [
        "ar rcT build/libcalc.a build/calc_add.o build/calc_mul.o",
        "ar rcs --plugin=plugin.so build/libcalc.a build/calc_add.o build/calc_mul.o",
        "ar -M script build/calc_add.o build/calc_mul.o",
        "ar rcs build/libcalc.a @members.rsp build/calc_add.o build/calc_mul.o",
        "ar rcs build/libcalc.a build/calc_add.o build/calc_mul.o > build/archive.log",
        "ar rcs build/libcalc.a build/calc_add.o build/calc_mul.o 2> build/archive.err",
        "ar rcs build/libcalc.a build/calc_add.o build/calc_mul.o < members.txt",
        "ar rcs build/libcalc.a build/calc_add.o build/calc_mul.o > build/archive.log 2>&1",
    ] {
        unchanged_without_unity(&format!(
            "{COMPILES}{archive}\nranlib build/libcalc.a\n\
             cc build/main.o build/libcalc.a -o build/unity-app\n"
        ));
    }
}

#[test]
fn archive_and_downstream_link_keep_extra_inputs_and_required_dependencies() {
    if !available(&["cc", "ar", "ranlib"]) {
        return;
    }
    let root = workspace();
    fs::write(root.path().join("seed.txt"), b"required library input\n").unwrap();
    let commands = COMMANDS.replace(
        "ranlib build/libcalc.a\n",
        "cp seed.txt build/archive-token\nranlib build/libcalc.a\n",
    );
    let commands = format!("{}\ncp seed.txt build/link-token\n", commands.trim_end());
    let mut transformed = graph(&commands, root.path());
    for (line, token, producer_line) in [(4, "build/archive-token", 5), (7, "build/link-token", 8)]
    {
        let job = transformed
            .jobs
            .iter_mut()
            .find(|job| job.source.line == line)
            .unwrap();
        job.spec.inputs.push(token.into());
        transformed.required_dependencies.push(RequiredDependency {
            producer_line,
            consumer_line: line,
            artifact: token.into(),
            reason: "extra downstream library prerequisite".into(),
        });
    }
    transformed.rebuild().unwrap();
    apply_unity(&mut transformed).unwrap();
    assert_eq!(execute(&transformed), EXPECTED);
    assert_unity_members(root.path(), false);
    for (line, token) in [(4, "build/archive-token"), (7, "build/link-token")] {
        let consumer = transformed
            .jobs
            .iter()
            .position(|job| job.source.line == line)
            .unwrap();
        let producer = transformed
            .jobs
            .iter()
            .position(|job| job.spec.outputs.contains(&token.into()))
            .unwrap();
        assert!(
            transformed.jobs[consumer]
                .spec
                .inputs
                .contains(&token.into())
        );
        assert!(
            transformed
                .edges
                .iter()
                .any(|edge| edge.from == producer && edge.to == consumer)
        );
        assert_eq!(
            fs::read(root.path().join(token)).unwrap(),
            b"required library input\n"
        );
    }
}

#[test]
fn restored_unity_archive_refuses_newly_thin_or_hardlinked_archives_before_mutation() {
    if !available(&["cc", "ar", "ranlib"]) {
        return;
    }
    for thin in [true, false] {
        let root = workspace();
        let original = graph(COMMANDS, root.path());
        assert_eq!(execute(&original), EXPECTED);
        let mut transformed = graph(COMMANDS, root.path());
        apply_unity(&mut transformed).unwrap();
        let saved = snapshot(&transformed);
        if thin {
            tool(
                root.path(),
                "ar",
                &[
                    "rcT",
                    "build/thin.a",
                    "build/calc_add.o",
                    "build/calc_mul.o",
                ],
            );
            fs::rename(
                root.path().join("build/thin.a"),
                root.path().join("build/libcalc.a"),
            )
            .unwrap();
        } else {
            fs::hard_link(
                root.path().join("build/libcalc.a"),
                root.path().join("build/alias.a"),
            )
            .unwrap();
        }
        let before = fs::read(root.path().join("build/libcalc.a")).unwrap();
        let mut registry = Registry::default();
        registry.register_type::<UnitySource>().unwrap();
        registry.register_type::<UnityArchive>().unwrap();
        let restored = BuildGraph::from_json(&saved, &registry).unwrap();
        assert!(run(&restored, &RunOptions::default()).is_err());
        assert_eq!(
            fs::read(root.path().join("build/libcalc.a")).unwrap(),
            before
        );
    }
}
