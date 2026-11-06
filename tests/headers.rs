use rusty_make::{
    commands::compiler::Compiler,
    graph::create_graph,
    headers::{HeaderTrie, add_header_dependencies, discover_headers},
    job::{FileAccess, Job, JobData, Producer},
    shell::ShellHints,
};
use std::{any::Any, fs};

#[derive(Debug)]
struct Generate {
    data: JobData,
}
impl Job for Generate {
    fn data(&self) -> &JobData {
        &self.data
    }
    fn data_mut(&mut self) -> &mut JobData {
        &mut self.data
    }
    fn execute(&self) -> anyhow::Result<()> {
        Ok(())
    }
}
fn writer(path: &str) -> Box<dyn Job> {
    Box::new(Generate {
        data: JobData::new(vec![FileAccess::write(path)]),
    })
}
fn compiler(include_dirs: &[&str]) -> Box<dyn Job> {
    let mut argv = vec!["cc".to_owned()];
    for directory in include_dirs {
        argv.push(format!("-I{directory}"));
    }
    argv.extend(["-c", "main.c", "-o", "main.o"].map(str::to_owned));
    Box::new(Compiler::parse(&argv, &ShellHints::default()).unwrap())
}
fn read_paths(job: &dyn Job) -> Vec<&str> {
    job.data()
        .accesses
        .iter()
        .filter(|access| access.access.reads())
        .map(|access| access.path.as_str())
        .collect()
}

#[test]
fn trie_selects_complete_component_subtrees_and_deduplicates() {
    let mut trie = HeaderTrie::default();
    for header in [
        "include/a.h",
        "include/sub/b.h",
        "include/sub/deep/c.h",
        "include-other/d.h",
        "include/a.h",
    ] {
        trie.insert(header);
    }
    assert_eq!(
        trie.headers_under("include"),
        vec!["include/a.h", "include/sub/b.h", "include/sub/deep/c.h"]
    );
    assert_eq!(
        trie.headers_under("include/sub"),
        vec!["include/sub/b.h", "include/sub/deep/c.h"]
    );
    assert_eq!(
        trie.headers_under("include/sub/"),
        vec!["include/sub/b.h", "include/sub/deep/c.h"]
    );
    assert!(trie.headers_under("incl").is_empty());
    assert!(trie.headers_under("missing").is_empty());
}

#[test]
fn existing_and_generated_headers_add_reads_without_modifying_argv() {
    let mut jobs = vec![
        writer("include/generated.h"),
        compiler(&["include", "include/sub", "include"]),
    ];
    let original_argv = (jobs[1].as_ref() as &dyn Any)
        .downcast_ref::<Compiler>()
        .unwrap()
        .argv
        .clone();
    add_header_dependencies(
        &mut jobs,
        [
            "include/existing.HPP",
            "include/sub/nested.h",
            "include-other/outside.h",
            "include/not-header.c",
        ]
        .map(str::to_owned),
    )
    .unwrap();
    assert_eq!(
        read_paths(jobs[1].as_ref()),
        vec![
            "main.c",
            "include/existing.HPP",
            "include/generated.h",
            "include/sub/nested.h"
        ]
    );
    assert_eq!(jobs[1].data().producers.len(), 1);
    assert_eq!(jobs[1].data().producers[0].input, "include/generated.h");
    assert_eq!(jobs[1].data().producers[0].job, 0);
    assert_eq!(
        (jobs[1].as_ref() as &dyn Any)
            .downcast_ref::<Compiler>()
            .unwrap()
            .argv,
        original_argv
    );
    let dag = create_graph(jobs).unwrap();
    assert_eq!(dag.waves, vec![vec![0], vec![1]]);
    assert_eq!(
        dag.unproduced[1],
        vec!["main.c", "include/existing.HPP", "include/sub/nested.h"]
    );
}

#[test]
fn enrichment_does_not_turn_a_compiler_output_into_an_initial_input() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.c").to_str().unwrap().to_owned();
    let include = root.path().join("include").to_str().unwrap().to_owned();
    let output = format!("{include}/generated.h");
    fs::write(&source, "int value;\n").unwrap();
    fs::create_dir(&include).unwrap();
    let argv = ["cc", "-E", "-I", &include, "-o", &output, &source].map(str::to_owned);
    let mut jobs: Vec<Box<dyn Job>> = vec![Box::new(
        Compiler::parse(&argv, &ShellHints::default()).unwrap(),
    )];
    add_header_dependencies(&mut jobs, Vec::new()).unwrap();
    create_graph(jobs).unwrap().check_inputs().unwrap();
}

#[test]
fn unique_future_generated_header_hoists_its_producer() {
    let mut jobs = vec![compiler(&["include"]), writer("include/generated.h")];
    add_header_dependencies(&mut jobs, Vec::new()).unwrap();
    assert_eq!(jobs[0].data().producers[0].job, 1);
    let dag = create_graph(jobs).unwrap();
    assert_eq!(dag.waves, vec![vec![1], vec![0]]);
    assert_eq!(dag.unproduced[0], vec!["main.c"]);
}

#[test]
fn latest_preceding_header_value_is_selected_not_all_future_writers() {
    let mut jobs = vec![
        writer("include/value.h"),
        writer("include/value.h"),
        compiler(&["include"]),
        writer("include/value.h"),
        writer("include/value.h"),
    ];
    add_header_dependencies(&mut jobs, Vec::new()).unwrap();
    assert_eq!(jobs[2].data().producers.len(), 1);
    assert_eq!(jobs[2].data().producers[0].job, 1);
    let dag = create_graph(jobs).unwrap();
    assert_eq!(dag.waves, vec![vec![0], vec![1], vec![2], vec![3], vec![4]]);
}

#[test]
fn ambiguous_future_header_writes_are_rejected_without_partial_changes() {
    let mut jobs = vec![
        compiler(&["include"]),
        writer("include/a.h"),
        writer("include/ambiguous.h"),
        writer("include/ambiguous.h"),
    ];
    let error = add_header_dependencies(&mut jobs, ["include/existing.h".to_owned()]).unwrap_err();
    assert!(error.to_string().contains("ambiguous future producers"));
    assert_eq!(read_paths(jobs[0].as_ref()), vec!["main.c"]);
    assert!(jobs[0].data().producers.is_empty());
}

#[test]
fn existing_generic_bindings_are_preserved_and_reapplication_is_idempotent() {
    let mut jobs = vec![
        compiler(&["include"]),
        writer("include/a.h"),
        writer("include/a.h"),
    ];
    jobs[0]
        .data_mut()
        .accesses
        .push(FileAccess::read("include/a.h"));
    jobs[0].data_mut().producers.push(Producer {
        input: "include/a.h".into(),
        job: 1,
    });
    add_header_dependencies(&mut jobs, Vec::new()).unwrap();
    add_header_dependencies(&mut jobs, Vec::new()).unwrap();
    assert_eq!(read_paths(jobs[0].as_ref()), vec!["main.c", "include/a.h"]);
    assert_eq!(jobs[0].data().producers.len(), 1);
    let dag = create_graph(jobs).unwrap();
    assert_eq!(dag.waves, vec![vec![1], vec![0], vec![2]]);
}

#[test]
fn non_compiler_jobs_do_not_receive_header_reads() {
    let mut jobs = vec![writer("include/a.h"), writer("other")];
    add_header_dependencies(&mut jobs, ["include/existing.h".to_owned()]).unwrap();
    assert!(jobs.iter().all(|job| read_paths(job.as_ref()).is_empty()));
}

#[test]
fn header_discovery_scans_only_include_directories_and_preserves_spelling() {
    let root = tempfile::tempdir().unwrap();
    let include = format!("{}/include", root.path().display());
    fs::create_dir_all(format!("{include}/sub")).unwrap();
    fs::write(format!("{include}/a.h"), "existing").unwrap();
    fs::write(format!("{include}/sub/b.HXX"), "nested").unwrap();
    fs::write(format!("{include}/source.c"), "not a header").unwrap();
    fs::write(root.path().join("outside.h"), "outside").unwrap();
    let missing = format!("{}/missing", root.path().display());
    let jobs = vec![compiler(&[&include, &missing])];
    assert_eq!(
        discover_headers(&jobs).unwrap(),
        vec![format!("{include}/a.h"), format!("{include}/sub/b.HXX")]
    );
}

#[cfg(unix)]
#[test]
fn header_discovery_does_not_follow_symlink_directory_cycles() {
    let root = tempfile::tempdir().unwrap();
    let include = root.path().to_str().unwrap();
    fs::write(root.path().join("a.h"), "existing").unwrap();
    std::os::unix::fs::symlink(root.path(), root.path().join("cycle")).unwrap();
    assert_eq!(
        discover_headers(&[compiler(&[include])]).unwrap(),
        vec![format!("{include}/a.h")]
    );
}
