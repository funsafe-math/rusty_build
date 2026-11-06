use rusty_make::{
    commands::rm::Rm,
    job::{Job, JobData},
    shell::ShellHints,
};
use std::{fs, path::Path};

fn tree(root: &Path) {
    fs::create_dir_all(root.join("nested/empty")).unwrap();
    fs::write(root.join("top"), b"top payload").unwrap();
    fs::write(root.join("nested/leaf"), b"nested payload").unwrap();
}

fn parse(args: &[&str]) -> anyhow::Result<Rm> {
    let argv = std::iter::once("rm")
        .chain(args.iter().copied())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    Rm::parse(&argv, &ShellHints::default())
}

fn path(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn recursive_spellings_remove_real_trees() {
    for option in ["--recursive", "-r", "-R", "-rf", "-fr", "-Rf", "-fR"] {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("tree");
        tree(&target);
        let job = parse(&[option, path(&target)]).unwrap();
        assert!(job.recursive);
        assert!(!job.data.unknown_effects);
        assert!(
            job.data
                .accesses
                .iter()
                .any(|access| access.path == path(&target) && access.access.writes())
        );
        job.execute().unwrap();
        assert!(!target.exists());
    }
}

#[test]
fn force_only_ignores_missing_paths_not_directories_or_invalid_components() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("tree");
    let missing = root.path().join("missing");
    tree(&target);
    for args in [
        vec![path(&target)],
        vec!["-f", path(&target)],
        vec![path(&missing)],
        vec!["-r", path(&missing)],
    ] {
        assert!(parse(&args).unwrap().execute().is_err(), "{args:?}");
        assert_eq!(
            fs::read(target.join("nested/leaf")).unwrap(),
            b"nested payload"
        );
    }
    for args in [
        vec!["-f", path(&missing)],
        vec!["-rf", path(&missing)],
        vec!["-f"],
    ] {
        parse(&args).unwrap().execute().unwrap();
    }
    assert!(parse(&[]).is_err());
    let file = root.path().join("file");
    fs::write(&file, b"not a directory").unwrap();
    assert!(
        parse(&["-rf", path(&file.join("child"))])
            .unwrap()
            .execute()
            .is_err()
    );
    assert_eq!(fs::read(file).unwrap(), b"not a directory");
}

#[test]
fn trailing_separator_preflights_all_operands_before_removing_anything() {
    let root = tempfile::tempdir().unwrap();
    let keep = root.path().join("keep");
    let file = root.path().join("file");
    fs::write(&keep, b"keep").unwrap();
    fs::write(&file, b"not a directory").unwrap();
    for option in ["-f", "-r", "-rf"] {
        let with_slash = format!("{}/", file.display());
        let job = parse(&[option, path(&keep), &with_slash]).unwrap();
        assert!(job.execute().is_err());
        assert_eq!(fs::read(&keep).unwrap(), b"keep");
        assert_eq!(fs::read(&file).unwrap(), b"not a directory");
    }
}

#[test]
fn root_and_dot_operands_are_rejected_without_filesystem_changes() {
    let root = tempfile::tempdir().unwrap();
    let keep = root.path().join("keep");
    fs::write(&keep, b"keep").unwrap();
    // Root operands are only parsed, never executed.
    for operand in [
        "/", "//", "/./", ".", "./", "..", "tree/.", "tree/..", "tree/../",
    ] {
        assert!(parse(&["-rf", path(&keep), operand]).is_err(), "{operand}");
        assert_eq!(fs::read(&keep).unwrap(), b"keep");
    }
    #[cfg(windows)]
    for operand in ["C:/", "C:\\", "C:/.", "C:/..", "\\\\server\\share\\"] {
        assert!(parse(&["-rf", operand]).is_err(), "{operand}");
    }
}

#[test]
fn direct_execution_also_preflights_unsafe_operands() {
    let root = tempfile::tempdir().unwrap();
    let keep = root.path().join("keep");
    fs::write(&keep, b"keep").unwrap();
    let job = Rm {
        force: true,
        recursive: true,
        paths: vec![path(&keep).to_owned(), ".".into()],
        data: JobData::default(),
    };
    assert!(job.execute().is_err());
    assert_eq!(fs::read(keep).unwrap(), b"keep");
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
    let paths = [
        p.join("tree"),
        p.join("dir-link"),
        p.join("slash-link"),
        p.join("file-link"),
        p.join("dangling-link"),
    ];
    let slash_link = format!("{}/", paths[2].display());
    parse(&[
        "-r",
        path(&paths[0]),
        path(&paths[1]),
        &slash_link,
        path(&paths[3]),
        path(&paths[4]),
    ])
    .unwrap()
    .execute()
    .unwrap();
    for operand in paths {
        assert!(fs::symlink_metadata(operand).is_err());
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
fn nonrecursive_removal_unlinks_directory_and_dangling_symlinks() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    tree(&p.join("target"));
    symlink(p.join("target"), p.join("dir-link")).unwrap();
    symlink(p.join("missing"), p.join("dangling-link")).unwrap();
    parse(&[path(&p.join("dir-link")), path(&p.join("dangling-link"))])
        .unwrap()
        .execute()
        .unwrap();
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
    let job = parse(&["-rf", path(&p.join("keep")), path(&p.join("alias/nested"))]).unwrap();
    assert!(job.execute().is_err());
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
    parse(&[
        path(&p.join("dir-link")),
        path(&p.join("dangling-dir-link")),
        path(&p.join("file-link")),
    ])
    .unwrap()
    .execute()
    .unwrap();
    let slash_link = format!("{}/", p.join("slash-link").display());
    parse(&["-rf", path(&p.join("tree")), &slash_link])
        .unwrap()
        .execute()
        .unwrap();
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
