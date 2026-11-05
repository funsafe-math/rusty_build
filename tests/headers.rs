use rusty_make::{
    graph::{BuildGraph, normalize},
    headers::add_generated_header_dependencies,
    parse_plan,
    registry::Registry,
    rewrite::{CopyChain, GraphRewrite},
};
use std::{collections::BTreeSet, fs, path::Path, process::Command};

fn edge(graph: &BuildGraph, from: usize, to: usize) -> bool {
    graph
        .edges
        .iter()
        .any(|edge| edge.from == from && edge.to == to)
}

#[test]
fn selected_sources_get_only_declared_headers_in_any_normalized_include_directory() {
    let root = tempfile::tempdir().unwrap();
    let headers = [
        "include space/a.H",
        "second/sub/b.HH",
        "second/c.HpP",
        "second/d.HXX",
        "second/e.H++",
        "second/f.INC",
        "second/g.INL",
        "second/h.IPP",
        "second/i.TPP",
        "second/j.TCC",
        "second/k.CUH",
    ];
    let mut text = String::from("touch");
    for path in headers
        .iter()
        .chain(["second/not-header.c", "second-extra/outside.h"].iter())
    {
        text.push_str(&format!(" '{path}'"));
    }
    text.push_str("\ncc -I'include space' -I'include space/../include space' -Isecond -c src/../src/special.c -o chosen.o\ncc -Isecond src/other.c src/special.c -o app\ncc -Isecond -c src/other.c -o other.o\n");
    let mut graph = parse_plan(&text, root.path(), &Registry::default()).unwrap();
    let original_argv: Vec<_> = graph
        .jobs
        .iter()
        .map(|job| job.spec.parsed["argv"].clone())
        .collect();
    let original_inputs = graph.jobs[3].spec.inputs.clone();
    let stats = add_generated_header_dependencies(&mut graph, |_, source| {
        source == Path::new("src/../src/special.c") || source == Path::new("src/special.c")
    })
    .unwrap();
    assert_eq!(
        (
            stats.selected_jobs,
            stats.added_inputs,
            stats.added_dependencies
        ),
        (2, 21, 21)
    );
    let expected: BTreeSet<_> = headers
        .iter()
        .map(|path| normalize(root.path(), Path::new(path)))
        .collect();
    let selected_inputs: BTreeSet<_> = graph.jobs[1].spec.inputs.iter().skip(1).cloned().collect();
    assert_eq!(selected_inputs, expected);
    assert_eq!(graph.jobs[3].spec.inputs, original_inputs);
    assert!(graph.required_dependencies.iter().all(|dependency| {
        dependency.producer_line == 1
            && [2, 3].contains(&dependency.consumer_line)
            && expected.contains(&dependency.artifact)
    }));
    assert!(edge(&graph, 0, 1) && edge(&graph, 0, 2));
    assert!(graph.jobs[1].spec.unknown_effects && !graph.jobs[1].spec.cacheable);
    assert_eq!(
        graph
            .jobs
            .iter()
            .map(|job| job.spec.parsed["argv"].clone())
            .collect::<Vec<_>>(),
        original_argv
    );
}

#[test]
fn every_writer_is_required_existing_input_aliases_are_reused_and_reapplication_is_exact() {
    let root = tempfile::tempdir().unwrap();
    let mut graph = parse_plan(
        "touch include/generated.h\ntouch include/./generated.h\ncc -Iinclude -c main.c -o main.o\ntouch include/generated.h\n",
        root.path(), &Registry::default(),
    ).unwrap();
    graph.jobs[2]
        .spec
        .inputs
        .push("include/../include/generated.h".into());
    graph.rebuild().unwrap();
    let stats = add_generated_header_dependencies(&mut graph, |_, _| true).unwrap();
    assert_eq!(
        (
            stats.selected_jobs,
            stats.added_inputs,
            stats.added_dependencies
        ),
        (1, 0, 3)
    );
    assert_eq!(
        graph
            .required_dependencies
            .iter()
            .map(|dependency| dependency.producer_line)
            .collect::<Vec<_>>(),
        vec![1, 2, 4]
    );
    assert!(edge(&graph, 0, 2) && edge(&graph, 1, 2) && edge(&graph, 3, 2));
    let header = normalize(root.path(), Path::new("include/generated.h"));
    assert_eq!(
        graph.reads[2]
            .iter()
            .find(|read| read.path == header)
            .unwrap()
            .version,
        3
    );
    let before = serde_json::to_value(&graph).unwrap();
    let again = add_generated_header_dependencies(&mut graph, |_, _| true).unwrap();
    assert_eq!(
        (
            again.selected_jobs,
            again.added_inputs,
            again.added_dependencies
        ),
        (1, 0, 0)
    );
    assert_eq!(serde_json::to_value(&graph).unwrap(), before);
}

#[test]
fn no_selected_sources_means_no_graph_changes_and_self_outputs_are_not_inputs() {
    let root = tempfile::tempdir().unwrap();
    let mut graph = parse_plan(
        "touch include/other.h\ncc -Iinclude -c main.c -o include/self.H\n",
        root.path(),
        &Registry::default(),
    )
    .unwrap();
    let before = serde_json::to_value(&graph).unwrap();
    let stats = add_generated_header_dependencies(&mut graph, |_, _| false).unwrap();
    assert_eq!(
        (
            stats.selected_jobs,
            stats.added_inputs,
            stats.added_dependencies
        ),
        (0, 0, 0)
    );
    assert_eq!(serde_json::to_value(&graph).unwrap(), before);
    let stats = add_generated_header_dependencies(&mut graph, |_, _| true).unwrap();
    assert_eq!((stats.added_inputs, stats.added_dependencies), (1, 1));
    assert_eq!(
        graph.jobs[1].spec.inputs,
        vec![
            Path::new("main.c").to_owned(),
            normalize(root.path(), Path::new("include/other.h"))
        ]
    );
    assert_eq!(graph.required_dependencies[0].producer_line, 1);
}

#[test]
fn known_future_writer_runs_before_the_selected_compiler_and_binds_its_final_header() {
    let root = tempfile::tempdir().unwrap();
    let mut graph = parse_plan(
        "cc -Iinclude -c main.c -o main.o\ncp template include/generated.h\n",
        root.path(),
        &Registry::default(),
    )
    .unwrap();
    assert!(edge(&graph, 0, 1));
    let stats =
        add_generated_header_dependencies(&mut graph, |_, source| source == Path::new("main.c"))
            .unwrap();
    assert_eq!(
        (
            stats.selected_jobs,
            stats.added_inputs,
            stats.added_dependencies
        ),
        (1, 1, 1)
    );
    assert!(edge(&graph, 1, 0));
    assert!(!edge(&graph, 0, 1));
    assert_eq!(graph.waves, vec![vec![1], vec![0]]);
    let header = normalize(root.path(), Path::new("include/generated.h"));
    assert_eq!(
        graph.reads[0]
            .iter()
            .find(|read| read.path == header)
            .unwrap()
            .version,
        1
    );
}

#[test]
fn unsafe_unknown_future_writer_rejects_the_cycle_and_restores_the_entire_graph() {
    let root = tempfile::tempdir().unwrap();
    let mut graph = parse_plan(
        "cc -Iinclude -c main.c -o main.o\ntouch include/generated.h\n",
        root.path(),
        &Registry::default(),
    )
    .unwrap();
    graph.jobs[1].spec.unknown_effects = true;
    graph.rebuild().unwrap();
    let before = serde_json::to_value(&graph).unwrap();
    let error = add_generated_header_dependencies(&mut graph, |_, _| true).unwrap_err();
    assert!(format!("{error:#}").to_lowercase().contains("cycle"));
    assert_eq!(serde_json::to_value(&graph).unwrap(), before);
    assert!(edge(&graph, 0, 1) && !edge(&graph, 1, 0));
}

#[test]
fn python_identity_alone_never_invents_generated_outputs_from_arguments_or_files() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("include")).unwrap();
    fs::write(root.path().join("include/existing.h"), "#define VALUE 1\n").unwrap();
    let mut registry = Registry::default();
    registry
        .add_python("header-generator", "generate.py")
        .unwrap();
    let mut graph = parse_plan(
        "python3 generate.py --output include/claimed.h\ncc -Iinclude -c main.c -o main.o\n",
        root.path(),
        &registry,
    )
    .unwrap();
    assert!(graph.jobs[0].spec.unknown_effects && graph.jobs[0].spec.outputs.is_empty());
    let before = serde_json::to_value(&graph).unwrap();
    let stats = add_generated_header_dependencies(&mut graph, |_, _| true).unwrap();
    assert_eq!(
        (
            stats.selected_jobs,
            stats.added_inputs,
            stats.added_dependencies
        ),
        (1, 0, 0)
    );
    assert_eq!(serde_json::to_value(&graph).unwrap(), before);
}

#[test]
fn required_source_identity_survives_copy_fusion_before_or_after_the_policy() {
    let root = tempfile::tempdir().unwrap();
    let text = "cp template intermediate.h\ncp intermediate.h include/final.H\ncc -Iinclude -c main.c -o main.o\n";
    for rewrite_first in [false, true] {
        let mut graph = parse_plan(text, root.path(), &Registry::default()).unwrap();
        if rewrite_first {
            assert_eq!(CopyChain.apply(&mut graph).unwrap().transformations, 1);
        }
        let stats = add_generated_header_dependencies(&mut graph, |_, _| true).unwrap();
        assert_eq!((stats.added_inputs, stats.added_dependencies), (1, 1));
        assert_eq!(
            graph.required_dependencies[0].producer_line,
            if rewrite_first { 1 } else { 2 }
        );
        assert_eq!(graph.required_dependencies[0].consumer_line, 3);
        if !rewrite_first {
            assert_eq!(CopyChain.apply(&mut graph).unwrap().transformations, 1);
        }
        assert_eq!(graph.jobs.len(), 2);
        assert_eq!(
            graph.provenance[0]
                .iter()
                .map(|job| job.source.line)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(edge(&graph, 0, 1));
        let before = serde_json::to_value(&graph).unwrap();
        let again = add_generated_header_dependencies(&mut graph, |_, _| true).unwrap();
        assert_eq!((again.added_inputs, again.added_dependencies), (0, 0));
        assert_eq!(serde_json::to_value(&graph).unwrap(), before);
    }
}

#[test]
fn cli_policy_runs_with_no_optimization_and_rebuild_all_and_executes_a_real_compiler() {
    if Command::new("cc").arg("--version").output().is_err() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::create_dir(p.join("include space")).unwrap();
    fs::write(p.join("template"), "#define VALUE 37\n").unwrap();
    fs::write(
        p.join("main.c"),
        "#include <generated.h>\nint main(void) { return VALUE == 37 ? 0 : 1; }\n",
    )
    .unwrap();
    fs::write(p.join("other.c"), "int other(void) { return 1; }\n").unwrap();
    fs::write(p.join("commands.txt"), "cc -I'include space' main.c -o app\ncp template 'include space/generated.h'\ncc -I'include space' -c other.c -o other.o\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rusty_make"))
        .current_dir(p)
        .args([
            "--commands",
            "commands.txt",
            "--generated-header-source",
            "^never$",
            "--generated-header-source",
            "^main\\.c$",
            "--no-optimize",
            "--rebuild-all",
            "--dump-plan",
            "graph.json",
            "-j",
            "3",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(Command::new(p.join("app")).status().unwrap().success());
    assert!(p.join("other.o").is_file());
    let graph = BuildGraph::from_json(
        &fs::read(p.join("graph.json")).unwrap(),
        &Registry::default(),
    )
    .unwrap();
    assert_eq!(graph.jobs.len(), 3);
    assert_eq!(graph.required_dependencies.len(), 1);
    assert_eq!(
        (
            graph.required_dependencies[0].producer_line,
            graph.required_dependencies[0].consumer_line
        ),
        (2, 1)
    );
    assert!(edge(&graph, 1, 0));
    let header = normalize(p, Path::new("include space/generated.h"));
    assert!(graph.jobs[0].spec.inputs.contains(&header));
    assert!(!graph.jobs[2].spec.inputs.contains(&header));
    assert_eq!(fs::read_to_string(&header).unwrap(), "#define VALUE 37\n");
}

#[test]
fn system_quote_and_after_include_directories_participate_in_the_policy() {
    let root = tempfile::tempdir().unwrap();
    let mut graph = parse_plan(
        "touch local/a.h system/b.h quoted/c.h after/d.h\ncc -I local -isystem system -iquotequoted -idirafter after -c main.c -o main.o\n",
        root.path(), &Registry::default(),
    ).unwrap();
    let stats =
        add_generated_header_dependencies(&mut graph, |_, source| source == Path::new("main.c"))
            .unwrap();
    assert_eq!((stats.added_inputs, stats.added_dependencies), (4, 4));
    for header in ["local/a.h", "system/b.h", "quoted/c.h", "after/d.h"] {
        assert!(
            graph.jobs[1]
                .spec
                .inputs
                .contains(&normalize(root.path(), Path::new(header)))
        );
    }
    assert_eq!(
        graph.jobs[1].spec.parsed["includes"],
        serde_json::json!(["local", "system", "quoted", "after"])
    );
    assert_eq!(
        graph.jobs[1].spec.parsed["argv"],
        serde_json::json!([
            "cc",
            "-I",
            "local",
            "-isystem",
            "system",
            "-iquotequoted",
            "-idirafter",
            "after",
            "-c",
            "main.c",
            "-o",
            "main.o"
        ])
    );
}

#[test]
fn header_policy_does_not_override_a_real_producer_input_conflict() {
    let root = tempfile::tempdir().unwrap();
    let mut graph = parse_plan(
        "cc -Iinclude -c main.c -o template\ncp template include/generated.h\n",
        root.path(),
        &Registry::default(),
    )
    .unwrap();
    let before = serde_json::to_value(&graph).unwrap();
    let error = add_generated_header_dependencies(&mut graph, |_, _| true).unwrap_err();
    assert!(format!("{error:#}").contains("cycle"));
    assert_eq!(serde_json::to_value(&graph).unwrap(), before);
}

#[test]
fn cli_rejects_cyclic_header_policy_before_executing_any_prefix() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(
        p.join("commands.txt"),
        "touch marker\ncc -Iinclude -c main.c -o template\ncp template include/generated.h\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rusty_make"))
        .current_dir(p)
        .args([
            "--commands",
            "commands.txt",
            "--generated-header-source",
            "^main\\.c$",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cycle"));
    assert!(!p.join("marker").exists());
    assert!(!p.join(".rusty_make").exists());
}
