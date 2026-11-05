use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, ensure};
use clap::Parser;
use rusty_make::{
    headers::add_generated_header_dependencies,
    job::{JobSpec, Operation},
    parse_plan,
    registry::Registry,
    scheduler::{RunOptions, run},
    shell::ShellCommand,
};
use serde_json::json;

const COMMAND_LOG: &str = include_str!("../fixtures/python_headers/commands.log");
const EXPECTED_OUTPUT: &str = "generated answer: 42\n";

// This parser models one known script, not arbitrary Python programs. Registering
// an opaque Python command with add_python would not declare these effects.
#[derive(Parser)]
#[command(disable_help_flag = true, disable_version_flag = true)]
struct GenerateHeaderArgs {
    script: PathBuf,
    template: PathBuf,
    #[arg(long)]
    output: PathBuf,
}

fn parse_generator(shell: &ShellCommand) -> Result<JobSpec> {
    let args = GenerateHeaderArgs::try_parse_from(
        std::iter::once(shell.program.as_str()).chain(shell.args.iter().map(String::as_str)),
    )?;
    ensure!(
        args.script == Path::new("tools/generate_header.py"),
        "the generator parser only models tools/generate_header.py"
    );
    let parsed = json!({
        "script": args.script,
        "template": args.template,
        "output": args.output,
    });
    Ok(JobSpec {
        kind: "generate-header".into(),
        operation: Operation::External,
        inputs: vec![args.script, args.template],
        outputs: vec![args.output],
        unknown_effects: false,
        cacheable: false,
        parsed,
    })
}

fn require_tool(program: &str, description: &str) -> Result<()> {
    let output = Command::new(program)
        .arg("--version")
        .output()
        .with_context(|| {
            format!(
                "prerequisite missing: {description}; install `{program}` and make it available on PATH"
            )
        })?;
    ensure!(
        output.status.success(),
        "prerequisite unavailable: `{program} --version` failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

fn main() -> Result<()> {
    require_tool("python3", "Python 3 is needed to generate the header")?;
    require_tool("cc", "a C compiler is needed to build the application")?;
    require_tool("bash", "Bash is needed to execute the original command log")?;

    // Neither generated files nor scheduler metadata ever touch the repository.
    let workspace = tempfile::tempdir().context("creating isolated example workspace")?;
    let root = workspace.path();
    for directory in ["tools", "templates", "src", "generated", "build"] {
        fs::create_dir(root.join(directory))
            .with_context(|| format!("preparing directory {directory}"))?;
    }
    for (path, contents) in [
        (
            "tools/generate_header.py",
            include_str!("../fixtures/python_headers/tools/generate_header.py"),
        ),
        (
            "templates/generated.h.in",
            include_str!("../fixtures/python_headers/templates/generated.h.in"),
        ),
        (
            "src/special.c",
            include_str!("../fixtures/python_headers/src/special.c"),
        ),
    ] {
        fs::write(root.join(path), contents).with_context(|| format!("copying fixture {path}"))?;
    }

    let mut registry = Registry::default();
    registry.register(
        "generate-header",
        &[r"^\s*python3\s+tools/generate_header\.py(?:\s|$)"],
        parse_generator,
    )?;
    // The compiler is deliberately BEFORE its generator in the fixture log.
    // Only this source opts into dependencies on generated headers in its -I dirs.
    let mut graph = parse_plan(COMMAND_LOG, root, &registry)?;
    let stats = add_generated_header_dependencies(&mut graph, |_, source| {
        source == Path::new("src/special.c")
    })?;
    ensure!(
        stats.selected_jobs == 1 && stats.added_inputs == 1 && stats.added_dependencies == 1,
        "expected one selected compiler, one header input and one prerequisite; got {stats:?}"
    );
    ensure!(
        graph.topological_waves()? == vec![vec![1], vec![0]],
        "the generated-header prerequisite must hoist Python before the compiler"
    );
    println!(
        "Generated-header dependencies: selected jobs={}, added inputs={}, added dependencies={}",
        stats.selected_jobs, stats.added_inputs, stats.added_dependencies
    );

    let report = run(
        &graph,
        &RunOptions {
            jobs: 2,
            ..Default::default()
        },
    )
    .context("generating the header and compiling the C application")?;
    ensure!(
        report.executed == 2,
        "expected both fixture commands to execute"
    );
    println!(
        "Scheduler: executed={}, skipped={}, batches={}",
        report.executed, report.skipped, report.batches
    );

    let output = Command::new(root.join("build/generated-app"))
        .current_dir(root)
        .output()
        .context("running the compiled C application")?;
    ensure!(
        output.status.success(),
        "compiled application failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).context("decoding application output")?;
    ensure!(
        stdout == EXPECTED_OUTPUT,
        "unexpected application output: expected {EXPECTED_OUTPUT:?}, got {stdout:?}"
    );
    print!("{stdout}");
    Ok(())
}
