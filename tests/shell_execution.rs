use rusty_make::{
    parse_plan,
    registry::Registry,
    scheduler::{RunOptions, run},
};
use std::fs;
#[test]
fn numeric_stdin_descriptor_never_becomes_an_extra_copy_operand() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("a"), "payload").unwrap();
    fs::write(p.join("input"), "stdin").unwrap();
    let graph = parse_plan("cp a b 0<input\n0<input cp a c\n", p, &Registry::default()).unwrap();
    run(&graph, &RunOptions::default()).unwrap();
    assert_eq!(fs::read_to_string(p.join("b")).unwrap(), "payload");
    assert_eq!(fs::read_to_string(p.join("c")).unwrap(), "payload");
}

#[cfg(unix)]
#[test]
fn unknown_created_symlink_ancestors_cannot_invalidate_native_hazard_model() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("source"), "new").unwrap();
    fs::create_dir(p.join("real")).unwrap();
    let graph = parse_plan(
        "ln -s real alias\ncp source alias/output\n",
        p,
        &Registry::default(),
    )
    .unwrap();
    assert!(run(&graph, &RunOptions::default()).is_err());
    assert!(!p.join("real/output").exists());
}

#[test]
fn literal_space_paths_and_sed_redirections_execute_without_flattening() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    fs::write(p.join("file with spaces"), "foo foo\n").unwrap();
    let graph = parse_plan(
        "cp \"file with spaces\" copied\nsed -e 's/foo/bar/g' copied > transformed\nsed 's/bar/baz/g' transformed > output\nsed -i 's/baz/qux/g' output\ncat < output > redirected\nprintf 'tail\\n' >> redirected\n",
        p, &Registry::default(),
    ).unwrap();
    run(
        &graph,
        &RunOptions {
            jobs: 4,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(p.join("transformed")).unwrap(),
        "bar bar\n"
    );
    assert_eq!(fs::read_to_string(p.join("output")).unwrap(), "qux qux\n");
    assert_eq!(
        fs::read_to_string(p.join("redirected")).unwrap(),
        "qux qux\ntail\n"
    );
}
