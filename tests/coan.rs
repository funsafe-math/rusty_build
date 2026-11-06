use std::fs;
use std::path::Path;
use std::process::Command;

use rusty_make::commands::coan::Coan;
use rusty_make::job::{Access, FileAccess, Job, is_null_device};
use rusty_make::shell::ShellHints;

const CONDITIONAL: &str = "#ifdef FEATURE\nkept\n#else\ndropped\n#endif\n";
const BROKEN: &str = "#ifdef FEATURE\nunterminated\n";

// An explicit selection must work. No test substitutes a mock for real coan.
fn reference() -> Option<String> {
    let explicit = std::env::var("RUSTY_MAKE_COAN").ok();
    let program = explicit.clone().unwrap_or_else(|| "coan".into());
    match Command::new(&program).arg("--version").output() {
        Ok(output) if output.status.success() => Some(program),
        result if explicit.is_some() => panic!("RUSTY_MAKE_COAN must select real coan: {result:?}"),
        _ => {
            eprintln!("skipping real coan test: install coan or set RUSTY_MAKE_COAN");
            None
        }
    }
}

fn path(root: &Path, name: &str) -> String {
    root.join(name).to_str().unwrap().to_owned()
}

fn parse(program: &str, options: &[&str], inputs: &[&str], shell: ShellHints) -> Coan {
    let argv = std::iter::once(program)
        .chain(std::iter::once("source"))
        .chain(options.iter().copied())
        .chain(inputs.iter().copied())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut job = Coan::parse(&argv, &shell).unwrap();
    // Registry normally attaches these, including redirection accesses.
    for redirect in [&shell.stdout, &shell.stderr].into_iter().flatten() {
        if !is_null_device(redirect) {
            job.data.accesses.push(FileAccess::write(redirect));
        }
    }
    job.data.shell = shell;
    job
}

fn stdout(program: &str, options: &[&str], input: &str, output: &str) -> Coan {
    parse(
        program,
        options,
        &[input],
        ShellHints::new(Some(output.into()), None, false),
    )
}

fn batch(jobs: &[Coan]) -> anyhow::Result<()> {
    let items: Vec<&dyn Job> = jobs.iter().map(|job| job as &dyn Job).collect();
    jobs[0].execute_batch(&items)
}

#[test]
fn flat_metadata_retains_original_argv_and_generated_file_paths() {
    let hints = ShellHints::new(Some("generated/output.c".into()), None, false);
    let argv = ["tools/coan", "source", "-DFEATURE=1", "generated/input.c"].map(str::to_owned);
    let job = Coan::parse(&argv, &hints).unwrap();
    assert_eq!(job.argv, argv);
    assert_eq!(job.args, ["tools/coan", "source", "-DFEATURE=1"]);
    assert_eq!(job.inputs, ["generated/input.c"]);
    assert_eq!(job.outputs, ["generated/output.c"]);
    assert_eq!(job.data.accesses, [FileAccess::read("generated/input.c")]);
    assert!(!job.data.unknown_effects);
    let job = parse(
        "coan",
        &["--replace", "-DFEATURE=1"],
        &["generated.c"],
        ShellHints::default(),
    );
    assert_eq!(job.data.accesses, [FileAccess::read_write("generated.c")]);
    assert_eq!(job.data.accesses[0].access, Access::ReadWrite);
    assert!(!job.data.unknown_effects);
}

#[test]
fn unknown_effects_keep_exact_argv_and_known_missing_values_are_errors() {
    for options in [
        vec!["--future-option", "value"],
        vec!["--backup=.bak"],
        vec!["--recurse"],
        vec!["--file", "settings"],
    ] {
        let job = parse(
            "coan",
            &options,
            &["input.c"],
            ShellHints::new(Some("out.c".into()), None, false),
        );
        assert!(job.data.unknown_effects, "{options:?}");
        assert_eq!(job.argv[2..2 + options.len()], options);
    }
    for option in [
        "-D",
        "--define",
        "-U",
        "--undef",
        "-g",
        "--gag",
        "-k",
        "--discard",
        "-x",
        "--conflict",
        "--max-expansion",
        "-F",
        "--filter",
        "-f",
        "--file",
        "-b",
        "--backup",
        "--define=",
        "--filter=",
    ] {
        let argv = ["coan", "source", "--future-option", option].map(str::to_owned);
        let error = Coan::parse(&argv, &ShellHints::default()).unwrap_err();
        assert!(
            format!("{error:#}").contains("requires"),
            "{option}: {error:#}"
        );
    }
    let job = parse("coan", &["--replace"], &[], ShellHints::default());
    assert!(
        job.data.unknown_effects,
        "stdin filename lists are not a closed input set"
    );
}

#[test]
fn real_single_stdout_and_changed_exit_17_preserve_input() {
    let Some(coan) = reference() else { return };
    let root = tempfile::tempdir().unwrap();
    let input = path(root.path(), "input.c");
    let output = path(root.path(), "output.c");
    fs::write(&input, CONDITIONAL).unwrap();
    fs::write(&output, "old output must be truncated\n").unwrap();
    let reference = Command::new(&coan)
        .args(["source", "-DFEATURE=1", &input])
        .output()
        .unwrap();
    assert_eq!(reference.status.code(), Some(17));
    let job = stdout(&coan, &["-DFEATURE=1"], &input, &output);
    assert_eq!(
        job.argv,
        [coan.as_str(), "source", "-DFEATURE=1", input.as_str()].map(str::to_owned)
    );
    job.execute().unwrap();
    assert_eq!(fs::read(&output).unwrap(), reference.stdout);
    assert_eq!(fs::read_to_string(&input).unwrap(), CONDITIONAL);
}

#[test]
fn real_stdout_batch_copies_outputs_and_keeps_all_inputs_unchanged() {
    let Some(coan) = reference() else { return };
    let root = tempfile::tempdir().unwrap();
    let inputs = [
        path(root.path(), "first input.c"),
        path(root.path(), "second.c"),
    ];
    let outputs = [
        path(root.path(), "first output.c"),
        path(root.path(), "second output.c"),
    ];
    let jobs: Vec<_> = inputs
        .iter()
        .zip(&outputs)
        .map(|(input, output)| {
            fs::write(input, CONDITIONAL).unwrap();
            fs::write(output, "old\n").unwrap();
            stdout(&coan, &["-DFEATURE=1", "--discard=drop"], input, output)
        })
        .collect();
    batch(&jobs).unwrap();
    for (input, output) in inputs.iter().zip(&outputs) {
        assert_eq!(fs::read_to_string(input).unwrap(), CONDITIONAL);
        assert_eq!(fs::read_to_string(output).unwrap(), "kept\n");
    }
}

#[test]
fn real_in_place_single_and_batch_remain_supported() {
    let Some(coan) = reference() else { return };
    let root = tempfile::tempdir().unwrap();
    let inputs: Vec<_> = ["single.c", "first spaced.c", "second.c"]
        .into_iter()
        .map(|name| path(root.path(), name))
        .collect();
    for input in &inputs {
        fs::write(input, CONDITIONAL).unwrap();
    }
    parse(
        &coan,
        &["-r", "-DFEATURE=1"],
        &[&inputs[0]],
        ShellHints::default(),
    )
    .execute()
    .unwrap();
    let jobs: Vec<_> = inputs[1..]
        .iter()
        .map(|input| {
            parse(
                &coan,
                &["--replace", "-DFEATURE=1"],
                &[input],
                ShellHints::default(),
            )
        })
        .collect();
    batch(&jobs).unwrap();
    for input in inputs {
        assert_eq!(fs::read_to_string(input).unwrap(), "kept\n");
    }
}

#[cfg(unix)]
#[test]
fn real_compatible_stdout_batch_has_one_aggregate_failure() {
    let Some(coan) = reference() else { return };
    let root = tempfile::tempdir().unwrap();
    let bad = path(root.path(), "bad.c");
    let good = path(root.path(), "good.c");
    let bad_output = path(root.path(), "bad output.c");
    let good_output = path(root.path(), "good output.c");
    fs::write(&bad, BROKEN).unwrap();
    fs::write(&good, CONDITIONAL).unwrap();
    let jobs = [
        stdout(&coan, &["-K", "-DFEATURE=1"], &bad, &bad_output),
        stdout(&coan, &["-K", "-DFEATURE=1"], &good, &good_output),
    ];
    assert!(batch(&jobs).is_err());
    // Sequential execution would stop at bad.c. --keepgoing processing good.c
    // proves that the failure belongs to one actual external batch.
    assert_eq!(fs::read_to_string(good_output).unwrap(), "kept\n");
    assert_eq!(fs::read_to_string(bad).unwrap(), BROKEN);
    assert_eq!(fs::read_to_string(good).unwrap(), CONDITIONAL);
}

#[cfg(unix)]
#[test]
fn real_in_place_batch_also_returns_aggregate_error() {
    let Some(coan) = reference() else { return };
    let root = tempfile::tempdir().unwrap();
    let bad = path(root.path(), "bad.c");
    let good = path(root.path(), "good.c");
    fs::write(&bad, BROKEN).unwrap();
    fs::write(&good, CONDITIONAL).unwrap();
    let jobs = [
        parse(
            &coan,
            &["-r", "-K", "-DFEATURE=1"],
            &[&bad],
            ShellHints::default(),
        ),
        parse(
            &coan,
            &["-r", "-K", "-DFEATURE=1"],
            &[&good],
            ShellHints::default(),
        ),
    ];
    assert!(batch(&jobs).is_err());
    assert_eq!(fs::read_to_string(good).unwrap(), "kept\n");
}

#[test]
fn real_differing_arguments_failure_policy_and_stderr_do_not_fuse() {
    let Some(coan) = reference() else { return };
    let root = tempfile::tempdir().unwrap();
    for difference in ["args", "failure", "stderr"] {
        let bad = path(root.path(), &format!("bad-{difference}.c"));
        let good = path(root.path(), &format!("good-{difference}.c"));
        let bad_out = path(root.path(), &format!("bad-{difference}-out.c"));
        let good_out = path(root.path(), &format!("good-{difference}-out.c"));
        fs::write(&bad, BROKEN).unwrap();
        fs::write(&good, CONDITIONAL).unwrap();
        let first = stdout(&coan, &["-K", "-DFEATURE=1"], &bad, &bad_out);
        let mut second = stdout(
            &coan,
            if difference == "args" {
                &["-K", "-UFEATURE"]
            } else {
                &["-K", "-DFEATURE=1"]
            },
            &good,
            &good_out,
        );
        if difference == "failure" {
            second.data.shell.ignore_failure = true;
        }
        if difference == "stderr" {
            second.data.shell.stderr = Some("/dev/null".into());
        }
        assert!(batch(&[first, second]).is_err());
        assert!(
            !Path::new(&good_out).exists(),
            "{difference} must retain command boundaries"
        );
    }
}

#[test]
fn real_filename_sensitive_line_and_filter_options_keep_single_boundaries() {
    let Some(coan) = reference() else { return };
    let root = tempfile::tempdir().unwrap();
    for options in [
        vec!["-K", "-DFEATURE=1", "--line"],
        vec!["-K", "-DFEATURE=1", "--filter=c"],
    ] {
        let bad = path(root.path(), "bad.c");
        let good = path(root.path(), "good.c");
        let bad_out = path(root.path(), "bad.out");
        let good_out = path(root.path(), "good.out");
        fs::write(&bad, BROKEN).unwrap();
        fs::write(&good, CONDITIONAL).unwrap();
        let jobs = [
            stdout(&coan, &options, &bad, &bad_out),
            stdout(&coan, &options, &good, &good_out),
        ];
        assert!(batch(&jobs).is_err());
        assert!(!Path::new(&good_out).exists());
    }
    let input = path(root.path(), "line-input.c");
    let output = path(root.path(), "line-output.c");
    fs::write(&input, CONDITIONAL).unwrap();
    let expected = Command::new(&coan)
        .args(["source", "-DFEATURE=1", "--line", &input])
        .output()
        .unwrap();
    stdout(&coan, &["-DFEATURE=1", "--line"], &input, &output)
        .execute()
        .unwrap();
    assert_eq!(fs::read(output).unwrap(), expected.stdout);
}

#[test]
fn real_failure_policy_masks_status_start_and_redirection_errors() {
    let Some(coan) = reference() else { return };
    let root = tempfile::tempdir().unwrap();
    let input = path(root.path(), "bad.c");
    let output = path(root.path(), "out.c");
    fs::write(&input, BROKEN).unwrap();
    let mut job = stdout(&coan, &["-DFEATURE=1"], &input, &output);
    assert!(job.execute().is_err());
    job.data.shell.ignore_failure = true;
    job.execute().unwrap();
    let missing = "rusty-make-test-no-such-coan-executable";
    let mut job = parse(missing, &["--future-option"], &[], ShellHints::default());
    assert!(job.execute().is_err());
    job.data.shell.ignore_failure = true;
    job.execute().unwrap();
    let directory = path(root.path(), "directory");
    fs::create_dir(&directory).unwrap();
    let mut job = parse(
        &coan,
        &["--future-option"],
        &[],
        ShellHints::new(Some(directory), None, false),
    );
    assert!(job.execute().is_err());
    job.data.shell.ignore_failure = true;
    job.execute().unwrap();
}

#[test]
fn real_ignored_aggregate_error_is_success_without_modifying_inputs() {
    let Some(coan) = reference() else { return };
    let root = tempfile::tempdir().unwrap();
    let jobs: Vec<_> = [("bad.c", BROKEN), ("good.c", CONDITIONAL)]
        .into_iter()
        .map(|(name, content)| {
            let input = path(root.path(), name);
            let output = path(root.path(), &format!("out-{name}"));
            fs::write(&input, content).unwrap();
            let mut job = stdout(&coan, &["-K", "-DFEATURE=1"], &input, &output);
            job.data.shell.ignore_failure = true;
            job
        })
        .collect();
    batch(&jobs).unwrap();
    assert_eq!(fs::read_to_string(&jobs[1].outputs[0]).unwrap(), "kept\n");
    assert_eq!(fs::read_to_string(&jobs[0].inputs[0]).unwrap(), BROKEN);
}

#[test]
fn real_generated_inputs_follow_their_producers() {
    use rusty_make::commands::cp::Cp;
    let Some(coan) = reference() else { return };
    let root = tempfile::tempdir().unwrap();
    let template = path(root.path(), "template.c");
    fs::write(&template, CONDITIONAL).unwrap();
    let mut jobs: Vec<Box<dyn Job>> = Vec::new();
    let mut outputs = Vec::new();
    for name in ["one", "two"] {
        let generated = path(root.path(), &format!("{name} generated.c"));
        let output = path(root.path(), &format!("{name} transformed.c"));
        let argv = vec!["cp".into(), template.clone(), generated.clone()];
        jobs.push(Box::new(Cp::parse(&argv, &ShellHints::default()).unwrap()));
        jobs.push(Box::new(stdout(
            &coan,
            &["-DFEATURE=1"],
            &generated,
            &output,
        )));
        outputs.push(output);
    }
    let graph = rusty_make::graph::create_graph(jobs).unwrap();
    assert!(
        graph
            .edges
            .iter()
            .any(|edge| edge.from == 0 && edge.to == 1)
    );
    assert!(
        graph
            .edges
            .iter()
            .any(|edge| edge.from == 2 && edge.to == 3)
    );
    graph.check_inputs().unwrap();
    rusty_make::scheduler::execute(&graph, false).unwrap();
    for output in outputs {
        assert_eq!(fs::read_to_string(output).unwrap(), "kept\n");
    }
    assert_eq!(fs::read_to_string(template).unwrap(), CONDITIONAL);
}

#[test]
fn direct_guards_reject_aliases_and_nonregular_files_before_truncation() {
    let root = tempfile::tempdir().unwrap();
    let input = path(root.path(), "input.c");
    fs::write(&input, CONDITIONAL).unwrap();
    let error = stdout("coan", &["-DFEATURE=1"], &input, &input)
        .execute()
        .unwrap_err();
    assert!(format!("{error:#}").contains("alias"));
    assert_eq!(fs::read_to_string(&input).unwrap(), CONDITIONAL);
    let directory = path(root.path(), "directory");
    fs::create_dir(&directory).unwrap();
    let error = parse("coan", &["-r"], &[&directory], ShellHints::default())
        .execute()
        .unwrap_err();
    assert!(format!("{error:#}").contains("regular"));
    let alias = path(root.path(), "alias.c");
    fs::hard_link(&input, &alias).unwrap();
    #[cfg(unix)]
    {
        let error = parse("coan", &["-r"], &[&input], ShellHints::default())
            .execute()
            .unwrap_err();
        assert!(format!("{error:#}").contains("hard-link"));
    }
    let error = stdout("coan", &["-DFEATURE=1"], &input, &alias)
        .execute()
        .unwrap_err();
    assert!(format!("{error:#}").contains("alias"));
    assert_eq!(fs::read_to_string(&input).unwrap(), CONDITIONAL);
}

#[cfg(unix)]
#[test]
fn symlinked_files_and_parent_components_are_rejected() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let input = path(root.path(), "input.c");
    let alias = path(root.path(), "alias.c");
    fs::write(&input, CONDITIONAL).unwrap();
    symlink(&input, &alias).unwrap();
    let error = parse("coan", &["-r"], &[&alias], ShellHints::default())
        .execute()
        .unwrap_err();
    assert!(format!("{error:#}").contains("symlink"));
    let parent = root.path().join("linked");
    symlink(root.path(), &parent).unwrap();
    let output = path(&parent, "output.c");
    let error = stdout("coan", &["-DFEATURE=1"], &input, &output)
        .execute()
        .unwrap_err();
    assert!(format!("{error:#}").contains("symlink"));
    assert_eq!(fs::read_to_string(&input).unwrap(), CONDITIONAL);
}

#[cfg(unix)]
#[test]
fn real_quotes_and_newlines_use_original_argv_not_stdin_lists() {
    let Some(coan) = reference() else { return };
    let root = tempfile::tempdir().unwrap();
    let jobs: Vec<_> = [
        "quoted\"name.c",
        "line\nbreak.c",
        "tab\tname.c",
        "vertical\u{b}name.c",
        "form\u{c}name.c",
    ]
    .into_iter()
    .map(|name| {
        let input = path(root.path(), &format!("input-{name}"));
        let output = path(root.path(), &format!("output-{name}"));
        fs::write(&input, CONDITIONAL).unwrap();
        stdout(&coan, &["-DFEATURE=1"], &input, &output)
    })
    .collect();
    batch(&jobs).unwrap();
    for job in jobs {
        assert_eq!(fs::read_to_string(&job.outputs[0]).unwrap(), "kept\n");
        assert_eq!(fs::read_to_string(&job.inputs[0]).unwrap(), CONDITIONAL);
    }
}

#[test]
fn real_noop_suffixes_apply_their_shell_failure_policy() {
    let Some(coan) = reference() else { return };
    for suffix in ["true", ":", "cd ."] {
        let text = format!(
            "{} source --not-a-real-coan-option 2> /dev/null || {suffix}",
            shell_words::quote(&coan)
        );
        let parsed = rusty_make::shell::parse(&text).unwrap();
        assert!(parsed.shell.ignore_failure);
        let mut job = Coan::parse(&parsed.argv, &parsed.shell).unwrap();
        job.data.shell = parsed.shell;
        job.execute().unwrap();
    }
}

#[test]
fn real_conflicting_paths_keep_sequential_command_boundaries() {
    let Some(coan) = reference() else { return };
    let root = tempfile::tempdir().unwrap();
    let input = path(root.path(), "input.c");
    let intermediate = path(root.path(), "intermediate.c");
    let output = path(root.path(), "output.c");
    fs::write(&input, CONDITIONAL).unwrap();
    let jobs = [
        stdout(&coan, &["-DFEATURE=1"], &input, &intermediate),
        stdout(&coan, &["-DFEATURE=1"], &intermediate, &output),
    ];
    batch(&jobs).unwrap();
    assert_eq!(fs::read_to_string(output).unwrap(), "kept\n");
    assert_eq!(fs::read_to_string(input).unwrap(), CONDITIONAL);
}

#[test]
fn real_ignored_missing_input_does_not_skip_later_valid_job() {
    let Some(coan) = reference() else { return };
    let root = tempfile::tempdir().unwrap();
    let missing = path(root.path(), "missing.c");
    let input = path(root.path(), "input.c");
    let missing_output = path(root.path(), "missing-out.c");
    let output = path(root.path(), "output.c");
    fs::write(&input, CONDITIONAL).unwrap();
    let mut jobs = [
        stdout(&coan, &["-DFEATURE=1"], &missing, &missing_output),
        stdout(&coan, &["-DFEATURE=1"], &input, &output),
    ];
    for job in &mut jobs {
        job.data.shell.ignore_failure = true;
    }
    batch(&jobs).unwrap();
    assert_eq!(fs::read_to_string(output).unwrap(), "kept\n");
    assert_eq!(fs::read_to_string(input).unwrap(), CONDITIONAL);
}

#[test]
fn real_stdout_batches_preserve_filename_sensitive_conflict_text() {
    let Some(coan) = reference() else { return };
    let root = tempfile::tempdir().unwrap();
    for policy in ["--conflict=comment", "--conflict=error"] {
        let mut jobs = Vec::new();
        let mut expected = Vec::new();
        for index in 0..2 {
            let input = path(root.path(), &format!("input-{index}.c"));
            let output = path(root.path(), &format!("output-{index}.c"));
            fs::write(&input, "#undef FEATURE\n").unwrap();
            expected.push(
                Command::new(&coan)
                    .args(["source", "-DFEATURE=1", policy, &input])
                    .output()
                    .unwrap()
                    .stdout,
            );
            jobs.push(stdout(&coan, &["-DFEATURE=1", policy], &input, &output));
        }
        batch(&jobs).unwrap();
        for (job, bytes) in jobs.iter().zip(expected) {
            assert_eq!(fs::read(&job.outputs[0]).unwrap(), bytes);
        }
    }
}

#[test]
fn real_ignored_stdout_batch_errors_preserve_emitted_prefixes() {
    let Some(coan) = reference() else { return };
    let root = tempfile::tempdir().unwrap();
    let mut jobs = Vec::new();
    let mut expected = Vec::new();
    for index in 0..2 {
        let input = path(root.path(), &format!("broken-{index}.c"));
        let output = path(root.path(), &format!("output-{index}.c"));
        fs::write(&input, "before\n#ifdef FEATURE\nkept\n").unwrap();
        expected.push(
            Command::new(&coan)
                .args(["source", "-DFEATURE=1", &input])
                .output()
                .unwrap()
                .stdout,
        );
        let mut job = stdout(&coan, &["-DFEATURE=1"], &input, &output);
        job.data.shell.ignore_failure = true;
        jobs.push(job);
    }
    batch(&jobs).unwrap();
    for (job, bytes) in jobs.iter().zip(expected) {
        assert_eq!(fs::read(&job.outputs[0]).unwrap(), bytes);
    }
}
