use std::{fs, process::Command};

use anyhow::{Context, Result, ensure};
use rusty_make::{
    graph::create_graph, headers::add_header_dependencies, parse, registry::Registry,
    scheduler::execute,
};

const EXPECTED_OUTPUT: &str = "generated answer: 42\n";

fn require_tool(program: &str, description: &str) -> Result<()> {
    let output = Command::new(program)
        .arg("--version")
        .output()
        .with_context(|| format!("prerequisite missing: {description}; install `{program}` and make it available on PATH"))?;
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

    // Caller-supplied absolute fixture paths need no global chdir or path rewrite.
    // The compiler deliberately precedes the trusted script in the command log.
    let text = format!(
        "cc -I '{}/generated' '{}/src/special.c' -o '{}/build/generated-app'\npython3 '{}/tools/generate_header.py' '{}/templates/generated.h.in' --output '{}/generated/generated.h'\n",
        root.display(),
        root.display(),
        root.display(),
        root.display(),
        root.display(),
        root.display(),
    );
    // Default Python dispatch composes only the known GenerateHeadersPy handler;
    // unrelated scripts are not silently treated as generic Python commands.
    let mut jobs = parse(&text, &Registry::default())?;
    add_header_dependencies(&mut jobs, Vec::<String>::new())?;
    let graph = create_graph(jobs)?;
    ensure!(
        graph.waves == vec![vec![1], vec![0]],
        "the generated-header prerequisite must hoist Python before the compiler"
    );
    graph.check_inputs()?;
    execute(&graph, true).context("generating the header and compiling the C application")?;

    let output = Command::new(root.join("build/generated-app"))
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
