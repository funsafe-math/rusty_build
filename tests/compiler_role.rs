#![cfg(target_os = "linux")]

use rusty_make::{
    commands::compiler::Compiler, graph::create_graph, job::Job, parse, registry::Registry,
    scheduler::execute,
};
use std::{
    any::Any,
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
    process::Command,
};

fn compiler_registry() -> Registry {
    let mut registry = Registry::empty();
    registry
        .register(&["odd driver"], |argv, hints| {
            Ok(Some(Box::new(Compiler::parse(argv, hints)?) as Box<dyn Job>))
        })
        .unwrap();
    registry
}

fn installed_compiler() -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").expect("PATH is set"))
        .map(|directory| directory.join("cc"))
        .find(|candidate| {
            fs::metadata(candidate).is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
        .expect("the compiler role fixture requires cc")
}

#[test]
fn exact_custom_compiler_registration_preserves_spaced_program_and_file_paths() {
    let root = tempfile::tempdir().unwrap();
    let p = root.path();
    for directory in ["tool bin", "source files", "object files", "program files"] {
        fs::create_dir(p.join(directory)).unwrap();
    }
    let driver = p.join("tool bin/odd driver");
    symlink(installed_compiler(), &driver).unwrap();
    let source = p.join("source files/main unit.c");
    let object = p.join("object files/main unit.o");
    let app = p.join("program files/custom app");
    fs::write(
        &source,
        "#include <stdio.h>\nint main(void) { puts(\"custom compiler output\"); return 0; }\n",
    )
    .unwrap();
    let text = format!(
        "'{}' -c '{}' -o '{}'\n'{}' '{}' -o '{}'\n",
        driver.display(),
        source.display(),
        object.display(),
        driver.display(),
        object.display(),
        app.display()
    );
    let jobs = parse(&text, &compiler_registry()).unwrap();
    let compile = (jobs[0].as_ref() as &dyn Any)
        .downcast_ref::<Compiler>()
        .unwrap();
    assert_eq!(
        compile.argv,
        [
            driver.to_str().unwrap(),
            "-c",
            source.to_str().unwrap(),
            "-o",
            object.to_str().unwrap()
        ]
    );
    assert_eq!(compile.sources, [source.to_str().unwrap()]);
    assert!(compile.data.unknown_effects);
    let dag = create_graph(jobs).unwrap();
    assert!(dag.edges.iter().any(|edge| edge.from == 0 && edge.to == 1));
    dag.check_inputs().unwrap();
    execute(&dag, false).unwrap();
    let result = Command::new(&app).output().unwrap();
    assert!(result.status.success());
    assert_eq!(result.stdout, b"custom compiler output\n");
}

#[test]
fn compiler_executor_passes_every_original_argument_to_the_selected_program() {
    let root = tempfile::tempdir().unwrap();
    let driver = root.path().join("odd driver");
    fs::write(&driver, "#!/bin/sh\nprintf '%s\\n' \"$0\" \"$@\"\n").unwrap();
    fs::set_permissions(&driver, fs::Permissions::from_mode(0o755)).unwrap();
    let source = root.path().join("main unit.c");
    fs::write(&source, "source input").unwrap();
    let log = root.path().join("argv log");
    let text = format!(
        "'{}' --future-mode 'opaque argument' -I'include dir' -c '{}' -o 'out file.o' > '{}'\n",
        driver.display(),
        source.display(),
        log.display()
    );
    let jobs = parse(&text, &compiler_registry()).unwrap();
    let compile = (jobs[0].as_ref() as &dyn Any)
        .downcast_ref::<Compiler>()
        .unwrap();
    let expected = compile.argv.join("\n") + "\n";
    assert_eq!(compile.unparsed_args, ["--future-mode", "opaque argument"]);
    let dag = create_graph(jobs).unwrap();
    dag.check_inputs().unwrap();
    execute(&dag, false).unwrap();
    assert_eq!(fs::read_to_string(log).unwrap(), expected);
}

#[test]
fn custom_roles_are_explicit_and_duplicate_executable_aliases_are_rejected() {
    assert!(parse("'odd driver' -c input.c -o output.o\n", &Registry::empty()).is_err());
    let mut registry = compiler_registry();
    assert!(
        registry
            .register(&["odd driver"], |argv, hints| {
                Ok(Some(Box::new(Compiler::parse(argv, hints)?) as Box<dyn Job>))
            })
            .is_err()
    );
    assert!(parse("'odd driver' -c input.c -o output.o\n", &registry).is_ok());
}
