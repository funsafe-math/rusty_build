#[path = "../examples/support/unity_transform.rs"]
mod unity_transform;

use rusty_make::{
    commands::compiler::Compiler,
    graph::{BuildGraph, Edge, RequiredDependency},
    parse_plan,
    registry::Registry,
    scheduler::{RunOptions, run},
};
use std::{collections::BTreeSet, fs, path::Path, process::Command};
use unity_transform::{
    UnityArchive, UnitySource, apply_unity, apply_unity_with, safe_unity_groups,
};

fn available(tools: &[&str]) -> bool {
    tools.iter().all(|tool| {
        let available = Command::new(tool).arg("--version").output().is_ok();
        if !available {
            eprintln!("skipping unavailable binary tool: {tool}");
        }
        available
    })
}

fn workspace(names: &[&str]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("src")).unwrap();
    fs::create_dir(root.path().join("build")).unwrap();
    for (index, name) in names.iter().enumerate() {
        fs::write(
            root.path().join(format!("src/{name}.c")),
            format!("int {name}(void) {{ return {}; }}\n", index + 1),
        )
        .unwrap();
    }
    let declarations = names
        .iter()
        .map(|name| format!("int {name}(void);\n"))
        .collect::<String>();
    let sum = names
        .iter()
        .map(|name| format!("{name}()"))
        .collect::<Vec<_>>()
        .join(" + ");
    fs::write(
        root.path().join("src/main.c"),
        format!(
            "#include <stdio.h>\n{declarations}int main(void) {{ printf(\"%d\\n\", {sum}); return 0; }}\n"
        ),
    )
    .unwrap();
    root
}

fn compile(name: &str, flags: &str) -> String {
    format!("cc {flags} -c src/{name}.c -o build/{name}.o\n")
}

fn compiles(names: &[&str], flags: &str) -> String {
    names.iter().map(|name| compile(name, flags)).collect()
}

fn direct_commands(names: &[&str]) -> String {
    let objects = names
        .iter()
        .map(|name| format!("build/{name}.o"))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{}{}cc build/main.o {objects} -o build/unity-app\n",
        compiles(names, "-O2"),
        compile("main", "-O0")
    )
}

fn graph(commands: &str, root: &Path) -> BuildGraph {
    parse_plan(commands, root, &Registry::default()).unwrap()
}

fn snapshot(graph: &BuildGraph) -> Vec<u8> {
    serde_json::to_vec(graph).unwrap()
}

fn stem(path: &Path) -> String {
    path.file_stem().unwrap().to_str().unwrap().to_owned()
}

fn proposed_sources(graph: &BuildGraph) -> Vec<Vec<String>> {
    let before = snapshot(graph);
    let groups = safe_unity_groups(graph).unwrap();
    assert_eq!(safe_unity_groups(graph).unwrap(), groups);
    assert_eq!(snapshot(graph), before);
    assert!(groups.iter().all(|group| group.len() >= 2));
    assert!(groups.windows(2).all(|pair| pair[0][0] < pair[1][0]));
    groups
        .into_iter()
        .map(|members| {
            assert!(members.windows(2).all(|pair| pair[0] < pair[1]));
            members
                .into_iter()
                .map(|id| stem(&graph.jobs[id].typed::<Compiler>().unwrap().sources[0]))
                .collect()
        })
        .collect()
}

fn generated_sources(graph: &BuildGraph) -> Vec<Vec<String>> {
    let mut groups = graph
        .jobs
        .iter()
        .filter_map(|job| job.typed::<UnitySource>())
        .map(|source| {
            source
                .sources
                .iter()
                .map(|path| stem(path))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    groups.sort();
    groups
}

fn assert_planning_has_no_outputs(root: &Path) {
    assert!(!root.join(".rusty_make").exists());
    assert_eq!(fs::read_dir(root.join("build")).unwrap().count(), 0);
}

fn assert_provenance(original: &BuildGraph, transformed: &BuildGraph) {
    for (job, provenance) in transformed.jobs.iter().zip(&transformed.provenance) {
        let Some(compiler) = job.typed::<Compiler>() else {
            continue;
        };
        if !compiler.compile_only || provenance.len() == 1 {
            continue;
        }
        assert!(provenance.len() >= 2);
        assert!(transformed.jobs.iter().any(|generator| {
            generator
                .typed::<UnitySource>()
                .is_some_and(|source| compiler.sources.contains(&source.output))
        }));
        assert!(
            provenance
                .windows(2)
                .all(|pair| pair[0].source.line < pair[1].source.line)
        );
        assert_eq!(job.source.text, provenance[0].source.text);
        for source in provenance {
            let original = original
                .jobs
                .iter()
                .find(|job| job.source.line == source.source.line)
                .unwrap();
            assert_eq!(
                serde_json::to_vec(source).unwrap(),
                serde_json::to_vec(original).unwrap()
            );
            for input in &original.spec.inputs {
                assert!(job.spec.inputs.contains(input));
            }
        }
    }
}

fn assert_original_object_effects(original: &BuildGraph, transformed: &BuildGraph) {
    let fused_lines = transformed
        .provenance
        .iter()
        .filter(|originals| originals.len() > 1)
        .flatten()
        .map(|job| job.source.line)
        .collect::<BTreeSet<_>>();
    for job in &original.jobs {
        let Some(compiler) = job
            .typed::<Compiler>()
            .filter(|compiler| compiler.compile_only)
        else {
            continue;
        };
        for output in &compiler.outputs {
            assert_eq!(
                transformed.cwd.join(output).is_file(),
                !fused_lines.contains(&job.source.line),
                "unexpected original object effect: {}",
                output.display()
            );
        }
    }
    for job in &transformed.jobs {
        if let Some(compiler) = job
            .typed::<Compiler>()
            .filter(|compiler| compiler.compile_only)
        {
            assert!(
                compiler
                    .outputs
                    .iter()
                    .all(|path| transformed.cwd.join(path).is_file())
            );
        }
    }
}

fn execute(graph: &BuildGraph, expected: &[u8]) {
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
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    assert_eq!(output.stdout, expected);
}

fn restored(graph: &BuildGraph) -> BuildGraph {
    let mut registry = Registry::default();
    registry.register_type::<UnitySource>().unwrap();
    registry.register_type::<UnityArchive>().unwrap();
    let saved = snapshot(graph);
    let restored = BuildGraph::from_json(&saved, &registry).unwrap();
    assert_eq!(snapshot(&restored), saved);
    restored
}

fn assert_archive_members(graph: &BuildGraph) {
    for archive in graph
        .jobs
        .iter()
        .filter_map(|job| job.typed::<UnityArchive>())
    {
        let output = Command::new("ar")
            .args(["t", &archive.archive.data.argv[2]])
            .current_dir(&graph.cwd)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let mut actual = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let mut expected = archive.archive.data.argv[3..]
            .iter()
            .map(|path| {
                Path::new(path)
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned()
            })
            .collect::<Vec<_>>();
        actual.sort();
        expected.sort();
        assert_eq!(actual, expected);
        assert!(
            archive
                .stale_members
                .iter()
                .all(|member| !actual.contains(member))
        );
    }
}

#[test]
fn identical_prefixes_in_separate_libraries_form_independent_groups() {
    if !available(&["cc", "ar"]) {
        return;
    }
    let names = ["calc_a", "calc_b", "calc_c", "calc_d"];
    let root = workspace(&names);
    let commands = format!(
        "{}{}ar rcs build/libone.a build/calc_a.o build/calc_c.o\n\
         ar rcs build/libtwo.a build/calc_b.o build/calc_d.o\n\
         cc build/main.o build/libone.a build/libtwo.a -o build/unity-app\n",
        compiles(&names, "-O2"),
        compile("main", "-O0")
    );
    let original = graph(&commands, root.path());
    assert_eq!(
        proposed_sources(&original),
        [["calc_a", "calc_c"], ["calc_b", "calc_d"]]
    );
    let mut constant = original.clone();
    assert_eq!(
        apply_unity_with(&mut constant, |_, _| Some(()))
            .unwrap()
            .groups,
        2
    );
    assert_eq!(
        generated_sources(&constant),
        [["calc_a", "calc_c"], ["calc_b", "calc_d"]]
    );
    let mut transformed = original.clone();
    let stats = apply_unity(&mut transformed).unwrap();
    assert_eq!(
        (stats.groups, stats.original_compiles, stats.unity_compiles),
        (2, 4, 2)
    );
    assert_eq!(
        generated_sources(&transformed),
        [["calc_a", "calc_c"], ["calc_b", "calc_d"]]
    );
    assert_provenance(&original, &transformed);
    assert_planning_has_no_outputs(root.path());
    let restored = restored(&transformed);
    execute(&restored, b"10\n");
    assert_original_object_effects(&original, &restored);
    assert_archive_members(&restored);
    execute(&restored, b"10\n");
    assert_archive_members(&restored);
}

#[test]
fn constant_policy_cannot_bridge_interleaved_compiler_signatures() {
    if !available(&["cc"]) {
        return;
    }
    let names = ["calc_a", "calc_b", "calc_c", "calc_d", "calc_lone"];
    let root = workspace(&names);
    let commands = format!(
        "{}{}{}{}{}{}cc build/main.o build/calc_a.o build/calc_b.o build/calc_c.o build/calc_d.o build/calc_lone.o -o build/unity-app\n",
        compile("calc_a", "-O1"),
        compile("calc_b", "-O2"),
        compile("calc_c", "-O1"),
        compile("calc_d", "-O2"),
        compile("calc_lone", "-O3"),
        compile("main", "-O0")
    );
    let original = graph(&commands, root.path());
    assert_eq!(
        proposed_sources(&original),
        [["calc_a", "calc_c"], ["calc_b", "calc_d"]]
    );
    let mut transformed = original.clone();
    let mut visited = BTreeSet::new();
    let stats = apply_unity_with(&mut transformed, |job, source| {
        assert!(job.source.text.contains(source.to_str().unwrap()));
        assert!(visited.insert(stem(source)));
        Some(())
    })
    .unwrap();
    assert_eq!(stats.groups, 2);
    assert_eq!(
        visited,
        ["calc_a", "calc_b", "calc_c", "calc_d"]
            .into_iter()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
    );
    assert_eq!(
        generated_sources(&transformed),
        [["calc_a", "calc_c"], ["calc_b", "calc_d"]]
    );
    assert_provenance(&original, &transformed);
    assert_planning_has_no_outputs(root.path());
    let restored = restored(&transformed);
    execute(&restored, b"15\n");
    assert_original_object_effects(&original, &restored);
}

#[test]
fn individual_unmodeled_consumer_stays_standalone_beside_a_safe_pair() {
    if !available(&["cc", "cp"]) {
        return;
    }
    let names = ["calc_a", "calc_b", "calc_c"];
    let root = workspace(&names);
    let commands = direct_commands(&names).replace(
        "cc build/main.o",
        "cp build/calc_c.o build/copied.o\ncc build/main.o",
    );
    let original = graph(&commands, root.path());
    assert_eq!(proposed_sources(&original), [["calc_a", "calc_b"]]);
    let standalone = serde_json::to_vec(&original.jobs[2]).unwrap();
    let mut transformed = original.clone();
    let stats = apply_unity_with(&mut transformed, |_, _| Some(())).unwrap();
    assert_eq!(stats.original_compiles, 2);
    let untouched = transformed
        .jobs
        .iter()
        .find(|job| job.source.line == 3)
        .unwrap();
    assert_eq!(serde_json::to_vec(untouched).unwrap(), standalone);
    assert_eq!(generated_sources(&transformed), [["calc_a", "calc_b"]]);
    assert_provenance(&original, &transformed);
    assert_planning_has_no_outputs(root.path());
    let restored = restored(&transformed);
    execute(&restored, b"6\n");
    assert_original_object_effects(&original, &restored);
    assert_eq!(
        fs::read(root.path().join("build/copied.o")).unwrap(),
        fs::read(root.path().join("build/calc_c.o")).unwrap()
    );
}

#[test]
fn constant_policy_cannot_bridge_link_option_regions() {
    if !available(&["cc"]) {
        return;
    }
    let names = ["calc_a", "calc_b", "calc_c", "calc_d"];
    let root = workspace(&names);
    let commands = direct_commands(&names).replace(
        "build/calc_b.o build/calc_c.o",
        "build/calc_b.o -L build build/calc_c.o",
    );
    let original = graph(&commands, root.path());
    assert_eq!(
        proposed_sources(&original),
        [["calc_a", "calc_b"], ["calc_c", "calc_d"]]
    );
    let mut transformed = original.clone();
    assert_eq!(
        apply_unity_with(&mut transformed, |_, _| Some(()))
            .unwrap()
            .groups,
        2
    );
    assert_eq!(
        generated_sources(&transformed),
        [["calc_a", "calc_b"], ["calc_c", "calc_d"]]
    );
    let link = transformed
        .jobs
        .iter()
        .find_map(|job| {
            job.typed::<Compiler>()
                .filter(|compiler| !compiler.compile_only)
        })
        .unwrap();
    assert!(link.argv.windows(2).any(|args| args == ["-L", "build"]));
    assert_provenance(&original, &transformed);
    assert_planning_has_no_outputs(root.path());
    let restored = restored(&transformed);
    execute(&restored, b"10\n");
    assert_original_object_effects(&original, &restored);
}

#[test]
fn custom_subdivision_and_exclusion_refine_each_consumer_class() {
    if !available(&["cc", "ar"]) {
        return;
    }
    let names = [
        "alpha_a", "beta_a", "alpha_b", "beta_b", "omit_a", "alpha_c", "beta_c", "alpha_d",
        "beta_d", "omit_b",
    ];
    let root = workspace(&names);
    let objects = |names: &[&str]| {
        names
            .iter()
            .map(|name| format!("build/{name}.o"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let commands = format!(
        "{}{}ar rcs build/libone.a {}\nar rcs build/libtwo.a {}\n\
         cc build/main.o build/libone.a build/libtwo.a -o build/unity-app\n",
        compiles(&names, "-O2"),
        compile("main", "-O0"),
        objects(&names[..5]),
        objects(&names[5..])
    );
    let original = graph(&commands, root.path());
    let safe = proposed_sources(&original);
    assert_eq!(safe, [names[..5].to_vec(), names[5..].to_vec()]);
    let mut transformed = original.clone();
    let mut visited = BTreeSet::new();
    let stats = apply_unity_with(&mut transformed, |_, source| {
        let name = stem(source);
        assert!(visited.insert(name.clone()));
        let prefix = name.split('_').next().unwrap();
        (prefix != "omit").then(|| prefix.to_owned())
    })
    .unwrap();
    assert_eq!(
        (stats.groups, stats.original_compiles, stats.unity_compiles),
        (4, 8, 4)
    );
    assert_eq!(
        visited,
        names
            .into_iter()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
    );
    let generated = generated_sources(&transformed);
    assert_eq!(
        generated,
        [
            ["alpha_a", "alpha_b"],
            ["alpha_c", "alpha_d"],
            ["beta_a", "beta_b"],
            ["beta_c", "beta_d"]
        ]
    );
    for group in &generated {
        assert_eq!(
            safe.iter()
                .filter(|class| group.iter().all(|member| class.contains(member)))
                .count(),
            1
        );
    }
    assert_provenance(&original, &transformed);
    assert_planning_has_no_outputs(root.path());
    let restored = restored(&transformed);
    execute(&restored, b"55\n");
    assert_original_object_effects(&original, &restored);
    assert_archive_members(&restored);
}

#[test]
fn disabling_filename_subdivision_merges_differently_named_safe_sources() {
    if !available(&["cc"]) {
        return;
    }
    let names = ["alpha", "beta"];
    let root = workspace(&names);
    let original = graph(&direct_commands(&names), root.path());
    assert_eq!(proposed_sources(&original), [["alpha", "beta"]]);
    let mut default = original.clone();
    assert_eq!(apply_unity(&mut default).unwrap().groups, 0);
    assert_eq!(snapshot(&default), snapshot(&original));
    let mut transformed = original.clone();
    assert_eq!(
        apply_unity_with(&mut transformed, |_, _| Some(()))
            .unwrap()
            .groups,
        1
    );
    assert_eq!(generated_sources(&transformed), [["alpha", "beta"]]);
    assert_provenance(&original, &transformed);
    assert_planning_has_no_outputs(root.path());
    let restored = restored(&transformed);
    execute(&restored, b"3\n");
    assert_original_object_effects(&original, &restored);
}

#[test]
fn required_compile_policy_and_authored_edges_survive_partial_grouping_and_restore() {
    if !available(&["cc", "cp"]) {
        return;
    }
    let names = ["calc_a", "calc_b", "calc_c", "calc_d"];
    let root = workspace(&names);
    fs::write(root.path().join("seed.txt"), b"preserved prerequisite\n").unwrap();
    let commands = format!(
        "{}cp seed.txt build/first-token\ncp seed.txt build/second-token\n",
        direct_commands(&names)
    );
    let mut original = graph(&commands, root.path());
    original.jobs[1].spec.inputs.push("build/calc_a.o".into());
    original.required_dependencies.push(RequiredDependency {
        producer_line: 1,
        consumer_line: 2,
        artifact: "build/calc_a.o".into(),
        reason: "authored compile prerequisite".into(),
    });
    original.rebuild().unwrap();
    let first = original
        .jobs
        .iter()
        .position(|job| job.spec.outputs.contains(&"build/first-token".into()))
        .unwrap();
    let second = original
        .jobs
        .iter()
        .position(|job| job.spec.outputs.contains(&"build/second-token".into()))
        .unwrap();
    assert!(
        !original
            .edges
            .iter()
            .any(|edge| edge.from == first && edge.to == second)
    );
    original.edges.push(Edge {
        from: first,
        to: second,
        reason: "authored token ordering".into(),
    });
    original.waves = original.topological_waves().unwrap();
    assert_eq!(proposed_sources(&original), [["calc_c", "calc_d"]]);
    let mut transformed = original.clone();
    assert_eq!(
        apply_unity_with(&mut transformed, |_, _| Some(()))
            .unwrap()
            .groups,
        1
    );
    assert_eq!(
        transformed.required_dependencies,
        original.required_dependencies
    );
    for line in [1, 2] {
        let job = transformed
            .jobs
            .iter()
            .find(|job| job.source.line == line)
            .unwrap();
        assert_eq!(
            serde_json::to_vec(job).unwrap(),
            serde_json::to_vec(&original.jobs[line - 1]).unwrap()
        );
    }
    assert_provenance(&original, &transformed);
    assert_planning_has_no_outputs(root.path());
    let restored = restored(&transformed);
    let first = restored
        .jobs
        .iter()
        .position(|job| job.spec.outputs.contains(&"build/first-token".into()))
        .unwrap();
    let second = restored
        .jobs
        .iter()
        .position(|job| job.spec.outputs.contains(&"build/second-token".into()))
        .unwrap();
    assert!(
        restored
            .edges
            .iter()
            .any(|edge| edge.from == first && edge.to == second)
    );
    execute(&restored, b"10\n");
    assert_original_object_effects(&original, &restored);
    for token in ["build/first-token", "build/second-token"] {
        assert_eq!(
            fs::read(root.path().join(token)).unwrap(),
            b"preserved prerequisite\n"
        );
    }
}

#[test]
fn malformed_graphs_fail_transactionally_before_policy_or_file_effects() {
    let names = ["calc_a", "calc_b"];
    let root = workspace(&names);
    let original = graph(&direct_commands(&names), root.path());
    for broken_provenance in [false, true] {
        let mut malformed = original.clone();
        if broken_provenance {
            malformed.provenance.clear();
        } else {
            malformed.edges.push(Edge {
                from: malformed.jobs.len(),
                to: 0,
                reason: "invalid endpoint".into(),
            });
        }
        let before = snapshot(&malformed);
        assert!(safe_unity_groups(&malformed).is_err());
        let mut invoked = false;
        assert!(
            apply_unity_with(&mut malformed, |_, _| {
                invoked = true;
                Some(())
            })
            .is_err()
        );
        assert!(!invoked);
        assert_eq!(snapshot(&malformed), before);
        assert_planning_has_no_outputs(root.path());
    }
}
