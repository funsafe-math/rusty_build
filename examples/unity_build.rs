//! Run with `cargo run --example unity_build` (requires `cc` on PATH).
//!
//! The reusable, opt-in graph transform is in support/unity_transform.rs. No
//! normal CLI invocation enables it. Read its restrictions before selecting C
//! sources: identical flags do not prove static identifiers/macros unity-safe.
#[path = "support/unity_transform.rs"]
mod unity_transform;

use anyhow::{Context, Result, ensure};
use rusty_make::{
    commands::compiler::Compiler,
    graph::BuildGraph,
    parse_plan,
    registry::Registry,
    scheduler::{RunOptions, run},
};
use std::{fs, path::Path, process::Command};
use unity_transform::apply_unity;

const COMMANDS: &str = include_str!("../fixtures/unity/commands.log");
const EXPECTED: &str = "unity demo: 42 42\n";

fn prepare(root: &Path) -> Result<()> {
    fs::create_dir(root.join("src"))?;
    fs::create_dir(root.join("build"))?;
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
        fs::write(root.join("src").join(name), contents)?;
    }
    Ok(())
}
fn compiler_invocations(graph: &BuildGraph) -> usize {
    graph
        .jobs
        .iter()
        .filter(|job| job.typed::<Compiler>().is_some())
        .count()
}
fn execute(graph: &BuildGraph) -> Result<String> {
    run(
        graph,
        &RunOptions {
            jobs: 2,
            ..Default::default()
        },
    )?;
    let output = Command::new(graph.cwd.join("build/unity-app"))
        .current_dir(&graph.cwd)
        .output()
        .context("running the compiled C application")?;
    ensure!(
        output.status.success(),
        "C application failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?)
}
fn main() -> Result<()> {
    // Each build has its own directory, so neither cache nor stale objects can
    // disguise a missing unity compile or a wrongly retained original object.
    let standalone = tempfile::tempdir()?;
    let unity = tempfile::tempdir()?;
    prepare(standalone.path())?;
    prepare(unity.path())?;
    let registry = Registry::default();
    let original = parse_plan(COMMANDS, standalone.path(), &registry)?;
    let mut transformed = parse_plan(COMMANDS, unity.path(), &registry)?;
    let before = compiler_invocations(&transformed);
    let stats = apply_unity(&mut transformed)?;
    let after = compiler_invocations(&transformed);
    ensure!(
        stats.original_compiles == 2 && stats.unity_compiles == 1 && after < before,
        "expected two calc compiles to become one unity compile"
    );

    // Rejection is planning-only and transactional: not even the generated TU
    // exists, and the original graph's commands/provenance remain untouched.
    let mismatch = COMMANDS.replace("-O2 -c src/calc_mul.c", "-O0 -c src/calc_mul.c");
    let mut rejected = parse_plan(&mismatch, unity.path(), &registry)?;
    let snapshot = serde_json::to_vec(&rejected)?;
    let error = apply_unity(&mut rejected).expect_err("incompatible flags must not merge");
    ensure!(
        serde_json::to_vec(&rejected)? == snapshot,
        "rejection changed the graph"
    );
    println!("Unsafe merge rejected: {error}");

    let standalone_output = execute(&original)?;
    let unity_output = execute(&transformed)?;
    ensure!(
        standalone_output == EXPECTED && unity_output == standalone_output,
        "standalone and unity application behavior differed"
    );
    ensure!(
        !unity.path().join("build/calc_add.o").exists()
            && !unity.path().join("build/calc_mul.o").exists(),
        "unity must not fabricate original object outputs"
    );
    println!("Compiler invocations (including link): {before} standalone -> {after} unity");
    print!("{unity_output}");
    Ok(())
}
