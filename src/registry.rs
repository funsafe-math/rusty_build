use anyhow::{Context, Result, bail};
use regex::Regex;
use serde_json::json;

use crate::commands;
use crate::input::SourceCommand;
use crate::job::{ExecutionStrategy, Job, JobSpec, Operation};
use crate::shell::{self, ShellCommand};

pub const UNKNOWN_PREFIX: &str = "unknown: ";
pub type JobParser = fn(&ShellCommand) -> Result<JobSpec>;

struct Handler {
    kind: String,
    patterns: Vec<Regex>,
    parser: HandlerParser,
}

enum HandlerParser {
    Registered(JobParser),
    ExternalTool,
    BinaryTool,
    Python { script: String },
}

/// Ordered registration is part of ambiguity diagnostics: each matching regex
/// counts, including multiple patterns belonging to the same handler.
pub struct Registry {
    handlers: Vec<Handler>,
}

impl Default for Registry {
    fn default() -> Self {
        let mut registry = Self {
            handlers: Vec::new(),
        };
        for (kind, programs, parser) in [
            ("cp", "cp", commands::copy as JobParser),
            ("mkdir", "mkdir", commands::mkdir as JobParser),
            ("touch", "touch", commands::touch as JobParser),
            ("rm", "rm", commands::remove as JobParser),
            ("mv", "mv", commands::move_file as JobParser),
            ("sed", "sed", commands::sed as JobParser),
            (
                "compiler",
                "(?:cc|c\\+\\+|gcc|g\\+\\+|clang|clang\\+\\+)(?:-[0-9]+(?:\\.[0-9]+)*)?",
                commands::compiler as JobParser,
            ),
        ] {
            let pattern = command_pattern(programs);
            registry
                .register(kind, &[&pattern], parser)
                .expect("built-in regex is valid");
        }
        // Keep each tool identity distinct. Prefixes cover GNU target triples
        // and LLVM spellings; every family still registers exactly one regex.
        for (kind, programs) in [
            ("nm", "nm"),
            ("objdump", "objdump"),
            ("readelf", "readelf"),
            ("size", "size"),
            ("strings", "strings"),
            ("addr2line", "addr2line"),
            ("c++filt", r"(?:c\+\+filt|cxxfilt)"),
            ("objcopy", "objcopy"),
            ("strip", "strip"),
            ("ar", "ar"),
            ("ranlib", "ranlib"),
            ("as", "as"),
            ("ld", r"(?:ld(?:\.(?:bfd|gold|lld))?|lld|gold)"),
            ("elfedit", "elfedit"),
            ("dwp", "dwp"),
            ("readobj", "readobj"),
            ("dwarfdump", "dwarfdump"),
            ("symbolizer", "symbolizer"),
        ] {
            let programs =
                format!(r"(?:[A-Za-z0-9_+.]+-)*(?:{programs})(?:-[0-9]+(?:\.[0-9]+)*)?(?:\.exe)?");
            registry
                .register_handler(
                    kind,
                    &[&command_pattern(&programs)],
                    HandlerParser::BinaryTool,
                )
                .expect("built-in binary tool regex is valid");
        }
        registry
    }
}

fn command_pattern(programs: &str) -> String {
    format!(
        r#"^\s*(?:(?:[^\s'";|&<>]*/)?(?:{programs})|'(?:[^']*/)?(?:{programs})'|"(?:[^"]*/)?(?:{programs})")(?:\s|$|[<>;&|])"#
    )
}

impl Registry {
    /// Add a custom typed parser. Bad regexes fail atomically, without leaving
    /// partially registered patterns behind. Overlaps are checked per command.
    pub fn register(&mut self, kind: &str, patterns: &[&str], parser: JobParser) -> Result<()> {
        self.register_handler(kind, patterns, HandlerParser::Registered(parser))
    }

    /// Register an opaque external tool without guessing its flag grammar or
    /// filesystem effects. All arguments and shell redirections are preserved.
    pub fn register_external(&mut self, kind: &str, patterns: &[&str]) -> Result<()> {
        self.register_handler(kind, patterns, HandlerParser::ExternalTool)
    }

    fn register_handler(
        &mut self,
        kind: &str,
        patterns: &[&str],
        parser: HandlerParser,
    ) -> Result<()> {
        if kind.is_empty() || patterns.is_empty() {
            bail!("registration requires a kind and at least one pattern");
        }
        let patterns = patterns
            .iter()
            .map(|pattern| {
                Regex::new(pattern)
                    .with_context(|| format!("invalid regex for handler {kind}: {pattern}"))
            })
            .collect::<Result<Vec<_>>>()?;
        self.handlers.push(Handler {
            kind: kind.into(),
            patterns,
            parser,
        });
        Ok(())
    }

    /// Configure one exact Python script identity independently of interpreter
    /// spelling. No Python script is implicitly trusted or declared cacheable.
    pub fn add_python(&mut self, identity: &str, script: &str) -> Result<()> {
        if identity.is_empty() || script.is_empty() {
            bail!("Python configuration requires an identity and script");
        }
        let interpreter = r"(?:python(?:[0-9]+(?:\.[0-9]+)*)?|py)";
        let prefix = command_pattern(interpreter);
        // Classify only regex-matched original text, including ordinary bare,
        // single-quoted and double-quoted script spellings. Other spellings
        // conservatively remain unknown, even if they decode to this script.
        let prefix = prefix
            .strip_suffix(r"(?:\s|$|[<>;&|])")
            .expect("command pattern suffix");
        let single = regex::escape(&format!("'{}'", script.replace('\'', "'\\''")));
        let double = regex::escape(&format!(
            "\"{}\"",
            script
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('$', "\\$")
                .replace('`', "\\`")
        ));
        let mut spellings = vec![single, double];
        if script
            .chars()
            .all(|c| !c.is_whitespace() && !"'\"\\$`;&|<>()*?[]{}~".contains(c))
        {
            spellings.push(regex::escape(script));
        }
        let alternatives = spellings.join("|");
        let pattern = format!(r"{prefix}\s+(?:{alternatives})(?:\s|$|[<>;&|])");
        self.handlers.push(Handler {
            kind: identity.into(),
            patterns: vec![Regex::new(&pattern)?],
            parser: HandlerParser::Python {
                script: script.into(),
            },
        });
        Ok(())
    }

    pub fn classify(&self, source: SourceCommand) -> Result<Job> {
        // Malformed shell is fatal even for unregistered commands.
        let shell = shell::parse(&source.text)
            .with_context(|| format!("invalid shell on line {}", source.line))?;
        let mut matches = Vec::new();
        for (handler_index, handler) in self.handlers.iter().enumerate() {
            for (pattern_index, pattern) in handler.patterns.iter().enumerate() {
                if pattern.is_match(&source.text) {
                    matches.push((handler_index, pattern_index));
                }
            }
        }
        if matches.len() > 1 {
            let descriptions = matches
                .iter()
                .map(|(handler, pattern)| {
                    format!(
                        "  {}[{}]: {}",
                        self.handlers[*handler].kind,
                        pattern,
                        self.handlers[*handler].patterns[*pattern].as_str()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            bail!(
                "ambiguous command classification on line {}: {}\n{} matching patterns\nMatching patterns:\n{descriptions}",
                source.line,
                source.text,
                matches.len()
            );
        }
        let Some((handler_index, _)) = matches.first() else {
            println!("{UNKNOWN_PREFIX}{}", source.text);
            return Ok(Job {
                source,
                shell,
                spec: barrier("unknown"),
                strategy: ExecutionStrategy::External,
            });
        };
        let handler = &self.handlers[*handler_index];
        let Some(command) = shell.as_ref() else {
            return Ok(Job {
                source,
                shell,
                spec: barrier(&handler.kind),
                strategy: ExecutionStrategy::External,
            });
        };
        let mut spec = match &handler.parser {
            HandlerParser::Registered(parser) => parser(command).with_context(|| {
                format!(
                    "{} parser failed on line {}: {}",
                    handler.kind, source.line, source.text
                )
            })?,
            HandlerParser::ExternalTool => commands::external_tool(&handler.kind, command),
            HandlerParser::BinaryTool => crate::binutils::parse(&handler.kind, command),
            HandlerParser::Python { script } => {
                if !is_python(&command.program) || command.args.first() != Some(script) {
                    bail!(
                        "Python parser failed on line {}: expected exact script {script}",
                        source.line
                    );
                }
                let mut spec = barrier(&handler.kind);
                spec.parsed = json!({"interpreter":command.program,"script":script,"args":&command.args[1..]});
                spec
            }
        };
        if command.stdin.is_some() || command.stdout.is_some() || command.stderr.is_some() {
            // A redirection adds known file effects, not an ordering barrier by
            // itself. Preserve precise external tool operands; native handlers
            // and opaque parsers still fall back to original-shell semantics.
            let precise_external =
                matches!(spec.operation, Operation::External) && !spec.unknown_effects;
            spec.operation = Operation::External;
            if !precise_external {
                spec.inputs.clear();
                spec.outputs.clear();
                spec.unknown_effects = true;
            }
            if let Some(path) = &command.stdin {
                spec.inputs.push(path.clone());
            }
            if let Some(path) = &command.stdout {
                spec.outputs.push(path.clone());
                if command.append_stdout {
                    spec.inputs.push(path.clone());
                }
            }
            if let Some(path) = &command.stderr {
                spec.outputs.push(path.clone());
            }
            spec.cacheable = false;
        }
        let strategy = if matches!(spec.operation, Operation::External) {
            ExecutionStrategy::External
        } else {
            ExecutionStrategy::Internal
        };
        Ok(Job {
            source,
            shell,
            spec,
            strategy,
        })
    }
}

fn is_python(program: &str) -> bool {
    let name = program.rsplit('/').next().unwrap_or(program);
    if name == "py" || name == "python" {
        return true;
    }
    name.strip_prefix("python").is_some_and(|version| {
        !version.is_empty()
            && version
                .split('.')
                .all(|part| !part.is_empty() && part.bytes().all(|c| c.is_ascii_digit()))
    })
}

fn barrier(kind: &str) -> JobSpec {
    JobSpec {
        kind: kind.into(),
        operation: Operation::External,
        inputs: Vec::new(),
        outputs: Vec::new(),
        unknown_effects: true,
        cacheable: false,
        parsed: json!({}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(text: &str) -> SourceCommand {
        SourceCommand {
            line: 1,
            text: text.into(),
            target: None,
        }
    }
    fn custom(_: &ShellCommand) -> Result<JobSpec> {
        Ok(barrier("custom"))
    }

    #[test]
    fn classifies_native_and_keeps_registered_errors_fatal() {
        let registry = Registry::default();
        let job = registry.classify(source("cp a b")).unwrap();
        assert!(matches!(job.spec.operation, Operation::Copy { .. }));
        assert!(job.spec.cacheable);
        assert!(registry.classify(source("cp -r a b")).is_err());
        assert!(
            registry
                .classify(source("unregistered 'unterminated"))
                .is_err()
        );
    }

    #[test]
    fn every_pattern_counts_in_deterministic_order() {
        let mut registry = Registry::default();
        registry
            .register("custom", &["^widget", "widget"], custom)
            .unwrap();
        let error = registry
            .classify(source("widget x"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("ambiguous command classification"));
        assert!(error.contains("widget x"));
        assert!(error.contains(
            "2 matching patterns\nMatching patterns:\n  custom[0]: ^widget\n  custom[1]: widget"
        ));
    }

    #[test]
    fn complex_commands_and_redirection_are_barriers() {
        let registry = Registry::default();
        let job = registry.classify(source("cp a b && touch done")).unwrap();
        assert_eq!(job.spec.kind, "cp");
        assert!(job.spec.unknown_effects && !job.spec.cacheable);
        assert_eq!(job.strategy, ExecutionStrategy::External);
        let job = registry.classify(source("cp a b > status")).unwrap();
        assert!(job.spec.unknown_effects && !job.spec.cacheable);
    }

    #[test]
    fn python_identity_handles_interpreters_and_quoted_paths() {
        let mut registry = Registry::default();
        registry
            .add_python("generate", "scripts/make data.py")
            .unwrap();
        for text in [
            "python 'scripts/make data.py' --out result",
            "/usr/bin/python3 \"scripts/make data.py\"",
            "py 'scripts/make data.py'",
        ] {
            let job = registry.classify(source(text)).unwrap();
            assert_eq!(job.spec.kind, "generate");
            assert!(job.spec.unknown_effects && !job.spec.cacheable);
        }
        assert_eq!(
            registry
                .classify(source("python other.py"))
                .unwrap()
                .spec
                .kind,
            "unknown"
        );
        assert_eq!(
            registry
                .classify(source("py scripts/make\\ data.py"))
                .unwrap()
                .spec
                .kind,
            "unknown"
        );
    }

    #[test]
    fn python_overlap_is_an_error_not_first_match_wins() {
        let mut registry = Registry::default();
        registry.add_python("one", "build.py").unwrap();
        registry.add_python("two", "build.py").unwrap();
        let error = registry
            .classify(source("python3 build.py"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("one[0]:") && error.contains("two[0]:"));
        assert!(error.matches("build\\.py").count() >= 2);
        let mut registry = Registry::default();
        registry.add_python("one", "build.py").unwrap();
        registry.register("broad", &["^python"], custom).unwrap();
        assert!(registry.classify(source("python build.py")).is_err());
    }

    #[test]
    fn unknown_prefix_is_exact() {
        assert_eq!(
            format!("{UNKNOWN_PREFIX}{}", "echo hello"),
            "unknown: echo hello"
        );
        let job = Registry::default().classify(source("echo hello")).unwrap();
        assert_eq!(job.spec.kind, "unknown");
        assert!(job.spec.unknown_effects && !job.spec.cacheable);
    }
}
