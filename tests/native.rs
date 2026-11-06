use rusty_make::{
    commands::{cp::Cp, mkdir::Mkdir, mv::Mv, rm::Rm, touch::Touch},
    graph::create_graph,
    job::{Access, Job},
    shell::ShellHints,
};
use std::{
    fs::{self, FileTimes, OpenOptions},
    path::Path,
    time::{Duration, SystemTime},
};

fn argv(program: &str, args: &[&str]) -> Vec<String> {
    std::iter::once(program)
        .chain(args.iter().copied())
        .map(str::to_owned)
        .collect()
}

fn path(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn cp(src: &Path, dst: &Path) -> Cp {
    Cp::parse(&argv("cp", &[path(src), path(dst)]), &ShellHints::default()).unwrap()
}

fn mv(src: &Path, dst: &Path) -> Mv {
    Mv::parse(&argv("mv", &[path(src), path(dst)]), &ShellHints::default()).unwrap()
}

#[test]
fn touch_creates_missing_and_updates_existing_file_without_truncating() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("payload");
    let job = Touch::parse(&argv("touch", &[path(&file)]), &ShellHints::default()).unwrap();
    job.execute().unwrap();
    assert!(file.is_file());
    fs::write(&file, b"payload remains intact").unwrap();
    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    OpenOptions::new()
        .write(true)
        .open(&file)
        .unwrap()
        .set_times(FileTimes::new().set_accessed(old).set_modified(old))
        .unwrap();
    job.execute().unwrap();
    assert_eq!(fs::read(&file).unwrap(), b"payload remains intact");
    assert!(fs::metadata(&file).unwrap().modified().unwrap() > old);
    assert!(
        job.data
            .accesses
            .iter()
            .all(|access| access.access == Access::Write)
    );
}

#[test]
fn mkdir_parents_creates_every_level_and_tolerates_existing_directories() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("one/two/three");
    let job = Mkdir::parse(
        &argv("mkdir", &["-p", path(&target)]),
        &ShellHints::default(),
    )
    .unwrap();
    job.execute().unwrap();
    job.execute().unwrap();
    assert!(target.is_dir());
    for directory in [
        root.path().join("one"),
        root.path().join("one/two"),
        target.clone(),
    ] {
        assert!(
            job.data
                .accesses
                .iter()
                .any(|access| access.path == path(&directory) && access.access.writes())
        );
    }
    let strict = Mkdir::parse(&argv("mkdir", &[path(&target)]), &ShellHints::default()).unwrap();
    assert!(strict.execute().is_err());
    let file = root.path().join("file");
    fs::write(&file, b"not a directory").unwrap();
    let invalid = Mkdir::parse(
        &argv("mkdir", &["-p", path(&file.join("child"))]),
        &ShellHints::default(),
    )
    .unwrap();
    assert!(invalid.execute().is_err());
    assert_eq!(fs::read(&file).unwrap(), b"not a directory");
}

#[test]
fn copy_and_move_preserve_payload_and_reject_directories_and_self_overwrites() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let copy = root.path().join("copy");
    let moved = root.path().join("moved");
    let directory = root.path().join("directory");
    fs::write(&source, b"payload").unwrap();
    fs::create_dir(&directory).unwrap();
    assert!(cp(&source, &source).execute().is_err());
    assert!(mv(&source, &source).execute().is_err());
    assert!(cp(&source, &directory).execute().is_err());
    assert!(mv(&source, &directory).execute().is_err());
    assert!(cp(&directory, &copy).execute().is_err());
    assert!(mv(&directory, &copy).execute().is_err());
    assert_eq!(fs::read(&source).unwrap(), b"payload");
    cp(&source, &copy).execute().unwrap();
    assert_eq!(fs::read(&copy).unwrap(), b"payload");
    fs::write(&moved, b"old longer contents").unwrap();
    mv(&copy, &moved).execute().unwrap();
    assert!(!copy.exists());
    assert_eq!(fs::read(moved).unwrap(), b"payload");
}

#[test]
fn copy_and_move_reject_hard_link_aliases_before_mutation() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let alias = root.path().join("alias");
    fs::write(&source, b"payload").unwrap();
    fs::hard_link(&source, &alias).unwrap();
    assert!(cp(&source, &alias).execute().is_err());
    assert!(mv(&source, &alias).execute().is_err());
    assert_eq!(fs::read(&source).unwrap(), b"payload");
    assert_eq!(fs::read(&alias).unwrap(), b"payload");
}

#[cfg(unix)]
#[test]
fn copy_retains_existing_destination_mode_and_hard_links() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let destination = root.path().join("destination");
    let alias = root.path().join("alias");
    fs::write(&source, b"new").unwrap();
    fs::write(&destination, b"old longer contents").unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(&destination, fs::Permissions::from_mode(0o640)).unwrap();
    fs::hard_link(&destination, &alias).unwrap();
    cp(&source, &destination).execute().unwrap();
    assert_eq!(
        fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert_eq!(fs::read(&destination).unwrap(), b"new");
    assert_eq!(fs::read(&alias).unwrap(), b"new");
}

#[test]
fn native_parsers_reject_unknown_options() {
    let hints = ShellHints::default();
    assert!(Cp::parse(&argv("cp", &["--unknown", "a", "b"]), &hints).is_err());
    assert!(Mkdir::parse(&argv("mkdir", &["--unknown", "a"]), &hints).is_err());
    assert!(Touch::parse(&argv("touch", &["--unknown", "a"]), &hints).is_err());
    assert!(Rm::parse(&argv("rm", &["--unknown", "a"]), &hints).is_err());
    assert!(Mv::parse(&argv("mv", &["--unknown", "a", "b"]), &hints).is_err());
    assert_eq!(
        Cp::parse(&argv("cp", &["--", "-source", "-destination"]), &hints)
            .unwrap()
            .src,
        "-source"
    );
}

#[test]
fn mutations_order_prior_readers_and_later_writers_without_unknown_barriers() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let middle = root.path().join("middle");
    let destination = root.path().join("destination");
    fs::write(&source, b"payload").unwrap();
    let copy = cp(&source, &middle);
    let movement = mv(&middle, &destination);
    let removal = Rm::parse(&argv("rm", &[path(&destination)]), &ShellHints::default()).unwrap();
    assert_eq!(movement.data.accesses[0].access, Access::ReadWrite);
    assert!(
        removal
            .data
            .accesses
            .iter()
            .all(|access| access.access == Access::Write)
    );
    let graph = create_graph(vec![Box::new(copy), Box::new(movement), Box::new(removal)]).unwrap();
    assert_eq!(graph.waves, vec![vec![0], vec![1], vec![2]]);
    graph.check_inputs().unwrap();
    for job in &graph.jobs {
        job.execute().unwrap();
    }
    assert_eq!(fs::read(source).unwrap(), b"payload");
    assert!(!middle.exists());
    assert!(!destination.exists());
}

#[test]
fn native_execution_opens_redirection_before_mutation_and_honors_failure_policy() {
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("redirect");
    let file = root.path().join("touched");
    fs::write(&output, b"must be truncated").unwrap();
    let mut hints = ShellHints::default();
    hints.stdout = Some(path(&output).to_owned());
    Touch::parse(&argv("touch", &[path(&file)]), &hints)
        .unwrap()
        .execute()
        .unwrap();
    assert_eq!(fs::read(&output).unwrap(), b"");
    assert!(file.exists());
    let missing_source = root.path().join("missing");
    let mut failing = cp(&missing_source, &root.path().join("copy"));
    failing.data.shell.ignore_failure = true;
    failing.execute().unwrap();
    assert!(!root.path().join("copy").exists());
    let blocked = root.path().join("blocked");
    hints.stdout = Some(path(&root.path().join("missing-parent/output")).to_owned());
    assert!(
        Touch::parse(&argv("touch", &[path(&blocked)]), &hints)
            .unwrap()
            .execute()
            .is_err()
    );
    assert!(!blocked.exists());
}

#[cfg(unix)]
#[test]
fn file_jobs_reject_symlink_paths_without_mutating_targets() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let target = root.path().join("target");
    let link = root.path().join("link");
    fs::write(&source, b"source").unwrap();
    fs::write(&target, b"target").unwrap();
    symlink(&target, &link).unwrap();
    assert!(cp(&source, &link).execute().is_err());
    assert!(cp(&link, &source).execute().is_err());
    assert!(mv(&source, &link).execute().is_err());
    assert!(mv(&link, &source).execute().is_err());
    assert!(
        Touch::parse(&argv("touch", &[path(&link)]), &ShellHints::default())
            .unwrap()
            .execute()
            .is_err()
    );
    assert_eq!(fs::read(source).unwrap(), b"source");
    assert_eq!(fs::read(target).unwrap(), b"target");
}
