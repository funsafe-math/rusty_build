use rusty_make::{
    commands::rm::Rm,
    graph::BuildGraph,
    job::{ExecutionContext, ExecutionStrategy, Job},
    parse_plan,
    registry::Registry,
    scheduler::{RunOptions, run},
};
use std::{fs, path::Path, process::Command};

fn tree(root: &Path) {
    fs::create_dir_all(root.join("nested/empty")).unwrap();
    fs::write(root.join("top"), b"top payload").unwrap();
    fs::write(root.join("nested/leaf"), b"nested payload").unwrap();
}

#[test]
fn recursive_spellings_use_native_handler_and_remove_real_trees() {
    for option in ["--recursive", "-r", "-R", "-rf", "-fr", "-Rf", "-fR"] {
        let root = tempfile::tempdir().unwrap();
        tree(&root.path().join("tree"));
        let text = format!("rm {option} tree\n");
        let graph = parse_plan(&text, root.path(), &Registry::default()).unwrap();
        let record = &graph.jobs[0];
        assert_eq!(record.strategy, ExecutionStrategy::Internal);
        assert!(record.typed::<Rm>().unwrap().recursive);
        assert!(record.spec.unknown_effects);
        assert!(!record.spec.cacheable);
        assert_eq!(record.source.text, text.trim_end());
        assert_eq!(record.shell.as_ref().unwrap().args, [option, "tree"]);
        let report = run(&graph, &RunOptions::default()).unwrap();
        assert_eq!((report.executed, report.skipped), (1, 0));
        assert!(!root.path().join("tree").exists());
    }
}

#[test]
fn force_only_ignores_missing_paths_and_never_enables_directory_removal() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    tree(&p.join("tree"));
    for text in [
        "rm tree\n",
        "rm -f tree\n",
        "rm missing\n",
        "rm -r missing\n",
    ] {
        let graph = parse_plan(text, p, &Registry::default()).unwrap();
        assert!(run(&graph, &RunOptions::default()).is_err(), "{text}");
        assert_eq!(
            fs::read(p.join("tree/nested/leaf")).unwrap(),
            b"nested payload"
        );
    }
    for text in ["rm -f missing\n", "rm -rf missing\n", "rm -f\n"] {
        let graph = parse_plan(text, p, &Registry::default()).unwrap();
        run(&graph, &RunOptions::default()).unwrap();
    }
    assert!(parse_plan("rm\n", p, &Registry::default()).is_err());
    fs::write(p.join("file"), b"not a directory").unwrap();
    let graph = parse_plan("rm -rf file/child\n", p, &Registry::default()).unwrap();
    assert!(run(&graph, &RunOptions::default()).is_err());
    assert_eq!(fs::read(p.join("file")).unwrap(), b"not a directory");
}

#[test]
fn trailing_separator_cannot_delete_a_regular_file_or_earlier_operand() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("keep"), b"keep").unwrap();
    fs::write(p.join("file"), b"not a directory").unwrap();
    for option in ["", "-f", "-r", "-rf"] {
        assert!(
            parse_plan(
                &format!("rm {option} keep file/\n"),
                p,
                &Registry::default(),
            )
            .is_err()
        );
        assert_eq!(fs::read(p.join("keep")).unwrap(), b"keep");
        assert_eq!(fs::read(p.join("file")).unwrap(), b"not a directory");
    }
}

#[test]
fn unsafe_operands_reject_the_whole_plan_without_removing_valid_operands() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("source"), b"source").unwrap();
    fs::write(p.join("keep"), b"keep").unwrap();
    // These checks only construct plans; root operands are never executed.
    for operand in [
        "/", "//", "/./", ".", "./", "..", "tree/.", "tree/..", "tree/../",
    ] {
        let text = format!("cp source prefix\nrm -rf keep {operand}\n");
        assert!(
            parse_plan(&text, p, &Registry::default()).is_err(),
            "{operand}"
        );
        assert_eq!(fs::read(p.join("keep")).unwrap(), b"keep");
        assert!(!p.join("prefix").exists());
    }
    #[cfg(windows)]
    for operand in ["C:/", "C:\\", "C:/.", "C:/.."] {
        assert!(parse_plan(&format!("rm -rf {operand:?}\n"), p, &Registry::default()).is_err());
    }
}

#[test]
fn direct_execution_preflights_every_operand_even_without_parsing() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("keep"), b"keep").unwrap();
    let graph = parse_plan("rm -f missing\n", p, &Registry::default()).unwrap();
    let job = Rm {
        force: true,
        recursive: true,
        paths: vec!["keep".into(), ".".into()],
    };
    let context = ExecutionContext {
        cwd: p,
        record: &graph.jobs[0],
    };
    assert!(job.execute(&context).is_err());
    assert_eq!(fs::read(p.join("keep")).unwrap(), b"keep");
}

#[cfg(unix)]
#[test]
fn recursive_removal_unlinks_symlinks_and_preserves_all_targets() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    tree(&p.join("target"));
    tree(&p.join("tree"));
    symlink(p.join("target"), p.join("tree/dir-link")).unwrap();
    symlink(p.join("target/top"), p.join("tree/file-link")).unwrap();
    symlink(p.join("tree"), p.join("tree/nested/cycle")).unwrap();
    symlink(p.join("target"), p.join("dir-link")).unwrap();
    symlink(p.join("target"), p.join("slash-link")).unwrap();
    symlink(p.join("target/top"), p.join("file-link")).unwrap();
    symlink(p.join("missing-target"), p.join("dangling-link")).unwrap();
    let graph = parse_plan(
        "rm -r tree dir-link slash-link/ file-link dangling-link\n",
        p,
        &Registry::default(),
    )
    .unwrap();
    run(&graph, &RunOptions::default()).unwrap();
    for name in [
        "tree",
        "dir-link",
        "slash-link",
        "file-link",
        "dangling-link",
    ] {
        assert!(fs::symlink_metadata(p.join(name)).is_err(), "{name}");
    }
    assert_eq!(fs::read(p.join("target/top")).unwrap(), b"top payload");
    assert_eq!(
        fs::read(p.join("target/nested/leaf")).unwrap(),
        b"nested payload"
    );
    assert!(p.join("target/nested/empty").is_dir());
}

#[cfg(unix)]
#[test]
fn nonrecursive_removal_also_unlinks_directory_and_dangling_symlinks() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    tree(&p.join("target"));
    symlink(p.join("target"), p.join("dir-link")).unwrap();
    symlink(p.join("missing"), p.join("dangling-link")).unwrap();
    let graph = parse_plan("rm dir-link dangling-link\n", p, &Registry::default()).unwrap();
    run(&graph, &RunOptions::default()).unwrap();
    assert!(fs::symlink_metadata(p.join("dir-link")).is_err());
    assert!(fs::symlink_metadata(p.join("dangling-link")).is_err());
    assert_eq!(fs::read(p.join("target/top")).unwrap(), b"top payload");
}

#[cfg(unix)]
#[test]
fn symlink_ancestors_are_rejected_before_any_operand_is_removed() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    tree(&p.join("target"));
    fs::write(p.join("keep"), b"keep").unwrap();
    symlink(p.join("target"), p.join("alias")).unwrap();
    assert!(parse_plan("rm -rf keep alias/nested\n", p, &Registry::default()).is_err());
    assert_eq!(fs::read(p.join("keep")).unwrap(), b"keep");
    assert_eq!(
        fs::read(p.join("target/nested/leaf")).unwrap(),
        b"nested payload"
    );
}

#[cfg(windows)]
#[test]
fn windows_directory_symlinks_are_unlinked_without_traversal() {
    use std::os::windows::fs::{symlink_dir, symlink_file};

    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    tree(&p.join("target"));
    match symlink_dir(p.join("target"), p.join("dir-link")) {
        Err(error) if error.raw_os_error() == Some(1314) => return,
        result => result.unwrap(),
    }
    tree(&p.join("tree"));
    symlink_dir(p.join("target"), p.join("tree/dir-link")).unwrap();
    symlink_dir(p.join("target"), p.join("slash-link")).unwrap();
    symlink_dir(p.join("missing"), p.join("dangling-dir-link")).unwrap();
    symlink_file(p.join("target/top"), p.join("file-link")).unwrap();
    let graph = parse_plan(
        "rm dir-link dangling-dir-link file-link\nrm -rf tree slash-link/\n",
        p,
        &Registry::default(),
    )
    .unwrap();
    run(&graph, &RunOptions::default()).unwrap();
    for name in [
        "tree",
        "dir-link",
        "slash-link",
        "dangling-dir-link",
        "file-link",
    ] {
        assert!(fs::symlink_metadata(p.join(name)).is_err(), "{name}");
    }
    assert_eq!(
        fs::read(p.join("target/nested/leaf")).unwrap(),
        b"nested payload"
    );
}

#[test]
fn serialized_recursive_jobs_execute_and_old_payloads_default_to_nonrecursive() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    tree(&p.join("tree"));
    let registry = Registry::default();
    let graph = parse_plan("rm -rf tree\n", p, &registry).unwrap();
    let encoded = serde_json::to_vec(&graph).unwrap();
    let restored = BuildGraph::from_json(&encoded, &registry).unwrap();
    assert!(restored.jobs[0].typed::<Rm>().unwrap().recursive);
    run(&restored, &RunOptions::default()).unwrap();
    assert!(!p.join("tree").exists());

    let graph = parse_plan("rm -f missing\n", p, &registry).unwrap();
    let mut encoded = serde_json::to_value(&graph).unwrap();
    encoded["jobs"][0]["spec"]["parsed"]
        .as_object_mut()
        .unwrap()
        .remove("recursive");
    let restored =
        BuildGraph::from_json(&serde_json::to_vec(&encoded).unwrap(), &registry).unwrap();
    assert!(!restored.jobs[0].typed::<Rm>().unwrap().recursive);
    run(&restored, &RunOptions::default()).unwrap();
}

#[test]
fn removal_is_never_cached_and_invalidates_later_directory_consumers() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("source"), b"first").unwrap();
    let text = "rm -rf tree\nmkdir -p tree/nested\ncp source tree/nested/leaf\ncp tree/nested/leaf result\n";
    let graph = parse_plan(text, p, &Registry::default()).unwrap();
    let report = run(&graph, &RunOptions::default()).unwrap();
    assert_eq!((report.executed, report.skipped), (4, 0));
    fs::write(p.join("tree/stale"), b"must be removed again").unwrap();
    fs::write(p.join("source"), b"second").unwrap();
    let report = run(&graph, &RunOptions::default()).unwrap();
    assert_eq!((report.executed, report.skipped), (4, 0));
    assert!(!p.join("tree/stale").exists());
    assert_eq!(fs::read(p.join("result")).unwrap(), b"second");
    assert_eq!(graph.waves, vec![vec![0], vec![1], vec![2], vec![3]]);
}

#[test]
fn cli_registers_recursive_rm_and_rejects_unsafe_plans_before_execution() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    tree(&p.join("tree"));
    fs::write(p.join("commands.txt"), "rm --recursive tree\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rusty_make"))
        .current_dir(p)
        .args(["--commands", "commands.txt", "--no-optimize"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!p.join("tree").exists());

    fs::write(p.join("keep"), b"keep").unwrap();
    fs::write(p.join("commands.txt"), "cp keep prefix\nrm -rf keep .\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rusty_make"))
        .current_dir(p)
        .args(["--commands", "commands.txt", "--no-optimize"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(fs::read(p.join("keep")).unwrap(), b"keep");
    assert!(!p.join("prefix").exists());
}
