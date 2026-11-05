#[path = "../examples/support/unity_transform.rs"]
mod unity_transform;

use rusty_make::{
    commands::compiler::Compiler,
    graph::{BuildGraph, RequiredDependency},
    job::Job,
    parse_plan,
    registry::Registry,
    scheduler::{RunOptions, run},
};
use std::{fs, path::Path, process::Command};
use unity_transform::{UnitySource, apply_unity};

const COMMANDS: &str = include_str!("../fixtures/unity/commands.log");
fn workspace() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("src")).unwrap();
    fs::create_dir(root.path().join("build")).unwrap();
    for (name, contents) in [
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
        fs::write(root.path().join("src").join(name), contents).unwrap();
    }
    root
}
fn graph(text: &str, root: &Path) -> BuildGraph {
    parse_plan(text, root, &Registry::default()).unwrap()
}
fn snapshot(graph: &BuildGraph) -> Vec<u8> {
    serde_json::to_vec(graph).unwrap()
}
fn rejected_unchanged(text: &str, expected: &str) {
    let root = workspace();
    let mut graph = graph(text, root.path());
    let before = snapshot(&graph);
    let error = apply_unity(&mut graph).unwrap_err();
    assert!(
        format!("{error:#}").contains(expected),
        "unexpected error: {error:#}"
    );
    assert_eq!(before, snapshot(&graph));
    assert!(!root.path().join(".rusty_make").exists());
    assert_eq!(fs::read_dir(root.path().join("build")).unwrap().count(), 0);
}
fn execute(graph: &BuildGraph, application: &str) -> (Vec<u8>, Vec<u8>, Option<i32>) {
    run(
        graph,
        &RunOptions {
            jobs: 2,
            ..Default::default()
        },
    )
    .unwrap();
    let output = Command::new(graph.cwd.join(application))
        .current_dir(&graph.cwd)
        .output()
        .unwrap();
    (output.stdout, output.stderr, output.status.code())
}

#[test]
fn unity_executes_fewer_compilers_with_identical_observable_output() {
    let standalone = workspace();
    let unity = workspace();
    let original = graph(COMMANDS, standalone.path());
    let mut transformed = graph(COMMANDS, unity.path());
    let original_commands = transformed
        .jobs
        .iter()
        .map(|j| j.source.text.clone())
        .collect::<Vec<_>>();
    let stats = apply_unity(&mut transformed).unwrap();
    assert_eq!(
        (stats.groups, stats.original_compiles, stats.unity_compiles),
        (1, 2, 1)
    );
    assert_eq!(
        original
            .jobs
            .iter()
            .filter(|j| j.typed::<Compiler>().is_some())
            .count(),
        4
    );
    assert_eq!(
        transformed
            .jobs
            .iter()
            .filter(|j| j.typed::<Compiler>().is_some())
            .count(),
        3
    );
    assert_eq!(
        transformed
            .jobs
            .iter()
            .filter(|j| j.typed::<UnitySource>().is_some())
            .count(),
        1
    );
    // Planning must have no filesystem effects.
    assert!(!unity.path().join(".rusty_make").exists());
    let combined = transformed
        .jobs
        .iter()
        .position(|j| {
            j.typed::<Compiler>()
                .is_some_and(|c| c.compile_only && c.sources[0].starts_with(".rusty_make"))
        })
        .unwrap();
    assert_eq!(
        transformed.provenance[combined]
            .iter()
            .map(|j| j.source.text.clone())
            .collect::<Vec<_>>(),
        original_commands[..2]
    );
    let compile = transformed.jobs[combined].typed::<Compiler>().unwrap();
    assert!(compile.inputs.contains(&"src/calc_add.c".into()));
    assert!(compile.inputs.contains(&"src/calc_mul.c".into()));
    assert_eq!(transformed.jobs[combined].source.text, original_commands[0]);
    let linked = transformed
        .jobs
        .iter()
        .find_map(|j| j.typed::<Compiler>().filter(|c| !c.compile_only))
        .unwrap();
    assert_eq!(
        linked
            .argv
            .iter()
            .filter(|arg| arg.ends_with("tu-0.o"))
            .count(),
        1
    );
    assert!(
        !linked
            .argv
            .iter()
            .any(|arg| arg == "build/calc_add.o" || arg == "build/calc_mul.o")
    );
    let baseline = execute(&original, "build/unity-app");
    assert_eq!(baseline, (b"unity demo: 42 42\n".to_vec(), vec![], Some(0)));
    assert_eq!(execute(&transformed, "build/unity-app"), baseline);
    assert!(!unity.path().join("build/calc_add.o").exists());
    assert!(!unity.path().join("build/calc_mul.o").exists());
    assert!(unity.path().join(".rusty_make/unity/tu-0.o").is_file());
}

#[test]
fn singleton_is_unchanged_even_when_another_group_is_transformed() {
    let root = workspace();
    let mut graph = graph(COMMANDS, root.path());
    let singleton = serde_json::to_vec(&graph.jobs[2]).unwrap();
    apply_unity(&mut graph).unwrap();
    let after = graph.jobs.iter().find(|j| j.source.line == 3).unwrap();
    assert_eq!(singleton, serde_json::to_vec(after).unwrap());
    let mut singleton_only = self::graph("cc -c src/main.c -o build/main.o\n", root.path());
    let before = snapshot(&singleton_only);
    assert_eq!(apply_unity(&mut singleton_only).unwrap().groups, 0);
    assert_eq!(before, snapshot(&singleton_only));
}

#[test]
fn mismatched_flags_and_compiler_reject_before_mutation() {
    rejected_unchanged(
        &COMMANDS.replace("-O2 -c src/calc_mul.c", "-O0 -c src/calc_mul.c"),
        "incompatible compiler/flags",
    );
    rejected_unchanged(
        &COMMANDS.replace("cc -O2 -c src/calc_mul.c", "gcc -O2 -c src/calc_mul.c"),
        "incompatible compiler/flags",
    );
    rejected_unchanged(
        &COMMANDS.replace("-O2 -c src/calc_mul.c", "-O2 -DOTHER=1 -c src/calc_mul.c"),
        "incompatible compiler/flags",
    );
}

#[test]
fn exact_argument_order_is_part_of_compatibility() {
    rejected_unchanged(
        &COMMANDS.replace("cc -O2 -c src/calc_mul.c", "cc -c -O2 src/calc_mul.c"),
        "incompatible compiler/flags",
    );
}

#[test]
fn all_link_consumers_are_rewritten_exactly_once() {
    let root = workspace();
    let commands = format!(
        "{COMMANDS}cc build/main.o build/calc_mul.o build/calc_add.o -o build/second-app\n"
    );
    let mut graph = graph(&commands, root.path());
    apply_unity(&mut graph).unwrap();
    let links = graph
        .jobs
        .iter()
        .filter_map(|j| j.typed::<Compiler>().filter(|c| !c.compile_only))
        .collect::<Vec<_>>();
    assert_eq!(links.len(), 2);
    for link in links {
        assert_eq!(
            link.argv.iter().filter(|a| a.ends_with("tu-0.o")).count(),
            1
        );
        assert_eq!(link.inputs.len(), 2);
    }
    assert_eq!(
        execute(&graph, "build/second-app"),
        (b"unity demo: 42 42\n".to_vec(), vec![], Some(0))
    );
}

#[test]
fn partial_duplicate_final_and_unmodeled_object_consumers_reject() {
    rejected_unchanged(
        &format!("{COMMANDS}cc build/calc_add.o build/main.o -o build/partial\n"),
        "whole group",
    );
    rejected_unchanged(
        &COMMANDS.replace(
            "cc build/calc_add.o",
            "cc build/calc_add.o build/calc_add.o",
        ),
        "whole group",
    );
    rejected_unchanged(
        &COMMANDS.lines().take(3).collect::<Vec<_>>().join("\n"),
        "final object outputs",
    );
    rejected_unchanged(
        &format!("{COMMANDS}cp build/calc_add.o build/copied.o\n"),
        "unmodeled object consumer",
    );
    rejected_unchanged(
        &format!("{COMMANDS}echo opaque\n"),
        "opaque or unknown-effect",
    );
}

#[test]
fn redirects_and_incomplete_compiler_metadata_reject() {
    rejected_unchanged(
        &COMMANDS.replace(
            "-o build/calc_add.o",
            "-o build/calc_add.o > build/compiler.log",
        ),
        "redirection",
    );
    rejected_unchanged(
        &COMMANDS.replace("-O2 -c src/calc_add.c", "-O2 -c src/calc_add.c -fPIC"),
        "complete compile-only",
    );
}

#[test]
fn required_source_line_policies_reject_without_losing_graph_requirements() {
    let root = workspace();
    let mut graph = graph(COMMANDS, root.path());
    graph.required_dependencies.push(RequiredDependency {
        producer_line: 1,
        consumer_line: 4,
        artifact: "build/calc_add.o".into(),
        reason: "explicit final-artifact policy".into(),
    });
    graph.rebuild().unwrap();
    let before = snapshot(&graph);
    let error = apply_unity(&mut graph).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("required header/artifact dependencies")
    );
    assert_eq!(before, snapshot(&graph));
    assert!(!root.path().join(".rusty_make").exists());
}

#[test]
fn multiple_groups_share_one_link_without_duplicate_unity_objects() {
    let root = workspace();
    let commands = "cc -c src/calc_add.c -o build/a.o\ncc -c src/calc_mul.c -o build/b.o\ncc -c src/other_one.c -o build/c.o\ncc -c src/other_two.c -o build/d.o\ncc build/a.o build/c.o build/b.o build/d.o -o build/app\n";
    let mut graph = graph(commands, root.path());
    let stats = apply_unity(&mut graph).unwrap();
    assert_eq!(stats.groups, 2);
    let link = graph.jobs.last().unwrap().typed::<Compiler>().unwrap();
    assert_eq!(link.inputs.len(), 2);
    for suffix in ["tu-0.o", "tu-1.o"] {
        assert_eq!(
            link.argv.iter().filter(|arg| arg.ends_with(suffix)).count(),
            1
        );
    }
}

#[test]
fn configured_arbitrary_compiler_names_are_accepted_but_not_mixed() {
    let root = workspace();
    let mut registry = Registry::empty();
    registry
        .register_with::<Compiler, _>(
            "custom-c-driver",
            &[r"^\s*(?:/opt/toolchains/my-c|other-c-driver)(?:\s|$)"],
            Compiler::parse,
        )
        .unwrap();
    let commands = COMMANDS.replace("cc ", "/opt/toolchains/my-c ");
    let mut graph = parse_plan(&commands, root.path(), &registry).unwrap();
    assert_eq!(apply_unity(&mut graph).unwrap().groups, 1);
    let unity = graph
        .jobs
        .iter()
        .find_map(|record| {
            record
                .typed::<Compiler>()
                .filter(|c| c.compile_only && c.sources[0].starts_with(".rusty_make"))
        })
        .unwrap();
    assert_eq!(unity.argv[0], "/opt/toolchains/my-c");
    let incompatible = commands.replace(
        "/opt/toolchains/my-c -O2 -c src/calc_mul.c",
        "other-c-driver -O2 -c src/calc_mul.c",
    );
    let mut graph = parse_plan(&incompatible, root.path(), &registry).unwrap();
    let before = snapshot(&graph);
    assert!(
        apply_unity(&mut graph)
            .unwrap_err()
            .to_string()
            .contains("incompatible compiler/flags")
    );
    assert_eq!(before, snapshot(&graph));
}

#[test]
fn persisted_unity_graph_rehydrates_generator_and_original_provenance() {
    let root = workspace();
    let mut registry = Registry::default();
    registry.register_type::<UnitySource>().unwrap();
    let mut graph = parse_plan(COMMANDS, root.path(), &registry).unwrap();
    apply_unity(&mut graph).unwrap();
    let saved = snapshot(&graph);
    let restored = BuildGraph::from_json(&saved, &registry).unwrap();
    assert_eq!(saved, snapshot(&restored));
    assert!(
        restored
            .jobs
            .iter()
            .any(|record| record.typed::<UnitySource>().is_some())
    );
    assert_eq!(
        execute(&restored, "build/unity-app"),
        (b"unity demo: 42 42\n".to_vec(), vec![], Some(0))
    );
}

#[test]
fn unrelated_required_dependencies_survive_unity_rewrite() {
    let root = workspace();
    let commands =
        format!("{COMMANDS}cp seed.txt build/seed.txt\ncp build/seed.txt build/final.txt\n");
    let mut graph = graph(&commands, root.path());
    graph.required_dependencies.push(RequiredDependency {
        producer_line: 5,
        consumer_line: 6,
        artifact: "build/seed.txt".into(),
        reason: "unrelated explicit prerequisite".into(),
    });
    graph.rebuild().unwrap();
    let requirements = graph.required_dependencies.clone();
    apply_unity(&mut graph).unwrap();
    assert_eq!(graph.required_dependencies, requirements);
}

#[test]
fn intervening_header_mutation_cannot_be_crossed_by_unity_compilation() {
    let root = workspace();
    fs::write(root.path().join("src/mutable.h"), "#define HEADER 1\n").unwrap();
    fs::write(root.path().join("next-header"), "#define HEADER 2\n").unwrap();
    fs::write(
        root.path().join("src/calc_add.c"),
        "#include \"mutable.h\"\nint calc_add(int a, int b) { return a + b + HEADER; }\n",
    )
    .unwrap();
    fs::write(
        root.path().join("src/calc_mul.c"),
        "#include \"mutable.h\"\nint calc_mul(int a, int b) { return a * b + HEADER; }\n",
    )
    .unwrap();
    let commands = COMMANDS.replace(
        "cc -O2 -c src/calc_mul.c",
        "cp next-header src/mutable.h\ncc -O2 -c src/calc_mul.c",
    );
    let mut original = graph(&commands, root.path());
    let before = snapshot(&original);
    assert!(apply_unity(&mut original).is_err());
    assert_eq!(snapshot(&original), before);
    assert!(!root.path().join(".rusty_make").exists());
    assert_eq!(
        execute(&original, "build/unity-app"),
        (b"unity demo: 43 44\n".to_vec(), vec![], Some(0))
    );
}

#[test]
fn link_effect_overlays_and_their_required_producer_survive_unity_replacement() {
    let root = workspace();
    fs::write(root.path().join("seed.txt"), "link prerequisite\n").unwrap();
    let commands = format!("{COMMANDS}cp seed.txt build/link-token\n");
    let mut graph = graph(&commands, root.path());
    graph.jobs[3].spec.inputs.push("build/link-token".into());
    graph.required_dependencies.push(RequiredDependency {
        producer_line: 5,
        consumer_line: 4,
        artifact: "build/link-token".into(),
        reason: "additional link prerequisite".into(),
    });
    graph.rebuild().unwrap();
    apply_unity(&mut graph).unwrap();
    let producer = graph
        .jobs
        .iter()
        .position(|job| job.source.line == 5)
        .unwrap();
    let link = graph
        .jobs
        .iter()
        .position(|job| {
            job.typed::<Compiler>()
                .is_some_and(|compiler| !compiler.compile_only)
        })
        .unwrap();
    assert!(
        graph.jobs[link]
            .spec
            .inputs
            .contains(&"build/link-token".into())
    );
    assert!(
        graph
            .edges
            .iter()
            .any(|edge| edge.from == producer && edge.to == link)
    );
    assert_eq!(
        execute(&graph, "build/unity-app"),
        (b"unity demo: 42 42\n".to_vec(), vec![], Some(0))
    );
    assert_eq!(
        fs::read(root.path().join("build/link-token")).unwrap(),
        b"link prerequisite\n"
    );
}
