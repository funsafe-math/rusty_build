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
use unity_transform::{apply_unity, apply_unity_with, safe_unity_groups};

const COMMANDS: &str = include_str!("../fixtures/unity/commands.log");
const LIBRARY_COMMANDS: &str = include_str!("../fixtures/unity/library-commands.log");
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
    let proposals = safe_unity_groups(&transformed)?;
    ensure!(
        proposals == vec![vec![0, 1, 2]],
        "expected all three ordinary compiles in one graph-compatible group"
    );
    let stats = apply_unity(&mut transformed)?;
    let after = compiler_invocations(&transformed);
    ensure!(
        stats.original_compiles == 2 && stats.unity_compiles == 1 && after < before,
        "expected two calc compiles to become one unity compile"
    );

    // Compiler-incompatible files remain standalone, without rejecting the plan.
    let mismatch = COMMANDS.replace("-O2 -c src/calc_mul.c", "-O0 -c src/calc_mul.c");
    let mut unmerged = parse_plan(&mismatch, unity.path(), &registry)?;
    let snapshot = serde_json::to_vec(&unmerged)?;
    ensure!(
        apply_unity(&mut unmerged)?.groups == 0,
        "incompatible files must not merge under the default filename policy"
    );
    ensure!(
        serde_json::to_vec(&unmerged)? == snapshot,
        "standalone fallback changed the graph"
    );
    println!("Incompatible candidates kept standalone; build planning continues");

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

    // A constant subdivision key opts out of the example's filename heuristic.
    let maximal = tempfile::tempdir()?;
    prepare(maximal.path())?;
    let mut maximal_graph = parse_plan(COMMANDS, maximal.path(), &registry)?;
    let maximal_stats = apply_unity_with(&mut maximal_graph, |_, _| Some(()))?;
    ensure!(
        maximal_stats.original_compiles == 3 && maximal_stats.unity_compiles == 1,
        "expected one whole graph-compatible group without filename subdivision"
    );
    ensure!(
        execute(&maximal_graph)? == EXPECTED,
        "unsubdivided unity changed application behavior"
    );
    println!(
        "Without filename subdivision: {before} standalone -> {} unity compiler invocations",
        compiler_invocations(&maximal_graph)
    );

    // Start with a real non-unity archive, then migrate that same build root.
    // Merely adding a unity member would leave stale original archive members.
    let library = tempfile::tempdir()?;
    prepare(library.path())?;
    let original_library = parse_plan(LIBRARY_COMMANDS, library.path(), &registry)?;
    let baseline = execute(&original_library)?;
    let mut unity_library = parse_plan(LIBRARY_COMMANDS, library.path(), &registry)?;
    apply_unity(&mut unity_library)?;
    ensure!(
        execute(&unity_library)? == baseline && baseline == EXPECTED,
        "static-library unity changed application behavior"
    );
    let members = Command::new("ar")
        .args(["t", "build/libcalc.a"])
        .current_dir(library.path())
        .output()
        .context("listing unity archive members")?;
    ensure!(
        members.status.success(),
        "cannot inspect unity archive members"
    );
    let members = String::from_utf8(members.stdout)?;
    ensure!(
        members == "tu-0.o\n",
        "stale original objects remain in archive: {members}"
    );
    println!(
        "Static library compiler invocations: {} standalone -> {} unity; archive members: {}",
        compiler_invocations(&original_library),
        compiler_invocations(&unity_library),
        members.trim()
    );
    Ok(())
}
