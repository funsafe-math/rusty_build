use crate::{
    compdb::CompilationDatabase,
    registry::Registry,
    rewrite::{CopyChain, GraphRewrite},
    scheduler::{self, RunOptions},
};
use anyhow::{Context, Result, bail};
use clap::{CommandFactory, FromArgMatches, Parser};
use std::{fs, path::PathBuf, time::Instant};

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Execute and optimize a GNU Make command log; without --commands, ask GNU Make to plan"
)]
pub struct Cli {
    #[arg(long, value_name = "FILE")]
    pub commands: Option<PathBuf>,
    #[arg(short = 'j', long, default_value_t = 1, value_name = "N")]
    pub jobs: usize,
    #[arg(long)]
    pub rebuild_all: bool,
    #[arg(long)]
    pub no_optimize: bool,
    #[arg(long, value_name = "FILE")]
    pub dump_graph: Option<PathBuf>,
    #[arg(long, value_name = "FILE")]
    pub dump_plan: Option<PathBuf>,
    #[arg(long, value_name = "FILE")]
    pub compile_commands: Option<PathBuf>,
    #[arg(long)]
    pub explain: bool,
    #[arg(long)]
    pub dry_run: bool,
    #[arg(short = 'v', long)]
    pub verbose: bool,
    /// Identify one script independently, e.g. --python-script generate=tools/generate.py
    #[arg(long, value_name = "IDENTITY=PATH")]
    pub python_script: Vec<String>,
    /// Pass one Make argument (use --make-arg=-f for options).
    #[arg(long, value_name = "ARG", allow_hyphen_values = true)]
    pub make_arg: Vec<String>,
    #[arg(value_name = "MAKE_ARG")]
    pub make_args: Vec<String>,
}
impl Cli {
    /// Preserve ordering across positional Make arguments and --make-arg values,
    /// particularly repeated variable assignments whose last value must win.
    pub fn parse_ordered() -> Self {
        let matches = Self::command().get_matches();
        let mut cli = Self::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());
        let mut forwarded: Vec<_> = matches
            .indices_of("make_arg")
            .into_iter()
            .flatten()
            .zip(std::mem::take(&mut cli.make_arg))
            .chain(
                matches
                    .indices_of("make_args")
                    .into_iter()
                    .flatten()
                    .zip(std::mem::take(&mut cli.make_args)),
            )
            .collect();
        forwarded.sort_by_key(|(index, _)| *index);
        cli.make_args = forwarded.into_iter().map(|(_, arg)| arg).collect();
        cli
    }

    pub fn run(self) -> Result<()> {
        if self.jobs == 0 {
            bail!("--jobs must be at least 1");
        }
        if self.commands.is_some() && (!self.make_args.is_empty() || !self.make_arg.is_empty()) {
            bail!("Make arguments cannot be combined with --commands");
        }
        let start = Instant::now();
        let cwd = std::env::current_dir()?;
        let mut registry = Registry::default();
        for config in &self.python_script {
            let (identity, script) = config
                .split_once('=')
                .context("--python-script requires IDENTITY=PATH")?;
            if identity.is_empty() || script.is_empty() {
                bail!("--python-script requires nonempty IDENTITY=PATH");
            }
            registry.add_python(identity, script)?;
        }
        let log = if let Some(path) = self.commands {
            crate::input::read_log(
                &fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?,
            )?
        } else {
            let mut args = self.make_arg;
            args.extend(self.make_args);
            crate::make::plan(&args, &cwd)?
        };
        let count = log.commands.len();
        let mut graph = crate::plan_log(log, &cwd, &registry)?;
        eprintln!("[PLAN] Parsed {count} commands");
        if !self.no_optimize {
            let stats = CopyChain.apply(&mut graph)?;
            eprintln!("[OPT] {} copy-chain transformations", stats.transformations);
        } else {
            graph
                .decisions
                .push("graph rewriting disabled by --no-optimize".into());
        }
        eprintln!(
            "[GRAPH] {} nodes, {} dependencies, {} waves",
            graph.jobs.len(),
            graph.edges.len(),
            graph.waves.len()
        );
        if self.explain || self.verbose {
            for (id, job) in graph.jobs.iter().enumerate() {
                eprintln!(
                    "[CLASSIFY] node {id} line {}: {} {:?} target={:?}",
                    job.source.line, job.spec.kind, job.strategy, job.source.target
                );
            }
            for edge in &graph.edges {
                eprintln!("[GRAPH] {} -> {}: {}", edge.from, edge.to, edge.reason);
            }
            for diagnostic in &graph.diagnostics {
                eprintln!("[INPUT] line {:?}: {}", diagnostic.line, diagnostic.text);
            }
            for decision in &graph.decisions {
                eprintln!("[PLAN] {decision}");
            }
        }
        if let Some(path) = self.dump_graph {
            fs::write(path, graph.dot())?;
        }
        if let Some(path) = self.dump_plan {
            fs::write(path, serde_json::to_vec_pretty(&graph)?)?;
        }
        if let Some(path) = self.compile_commands {
            CompilationDatabase::from_graph(&graph)?.save(&graph, &path)?;
        }
        if self.dry_run {
            for (wave, ids) in graph.waves.iter().enumerate() {
                println!("Wave {wave}:");
                for &id in ids {
                    println!(
                        "  [{id}] {}: {}",
                        graph.jobs[id].spec.kind, graph.jobs[id].source.text
                    );
                }
            }
        }
        let report = scheduler::run(
            &graph,
            &RunOptions {
                jobs: self.jobs,
                rebuild_all: self.rebuild_all,
                explain: self.explain || self.verbose,
                dry_run: self.dry_run,
            },
        )?;
        eprintln!("[CACHE] {} jobs unchanged", report.skipped);
        if self.dry_run {
            eprintln!(
                "[DRY] {} jobs would run in {} batches (cache decisions based on current filesystem)",
                report.planned, report.batches
            );
        } else {
            eprintln!(
                "[EXEC] {} jobs executed in {} batches",
                report.executed, report.batches
            );
        }
        eprintln!("[DONE] Completed in {:.3}s", start.elapsed().as_secs_f64());
        Ok(())
    }
}
