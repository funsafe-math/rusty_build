use crate::{
    commands::{self, external::External, python::Python},
    input::SourceCommand,
    job::{Job, JobRecord, JobSnapshot},
    shell::{self, ShellCommand},
};
use anyhow::{Context, Result, bail};
use regex::Regex;
use std::{
    any::{TypeId, type_name},
    collections::BTreeMap,
};

pub const UNKNOWN_PREFIX: &str = "unknown: ";
type ParseFactory = dyn Fn(SourceCommand, ShellCommand) -> Result<JobRecord> + Send + Sync;
struct Handler {
    classification: String,
    rust_type: TypeId,
    patterns: Vec<Regex>,
    parse: Box<ParseFactory>,
}
struct Decoder {
    rust_type: TypeId,
    rust_name: &'static str,
    version: u32,
    decode: fn(JobSnapshot) -> Result<JobRecord>,
}
/// Matchers classify original text; registered typed implementations own behavior.
/// Every matching regex counts, including overlaps inside one registration.
pub struct Registry {
    handlers: Vec<Handler>,
    decoders: BTreeMap<&'static str, Decoder>,
}
impl Default for Registry {
    fn default() -> Self {
        let mut registry = Self::empty();
        commands::register_builtins(&mut registry).expect("valid built-in command registrations");
        registry
    }
}
impl Registry {
    pub fn empty() -> Self {
        let mut registry = Self {
            handlers: Vec::new(),
            decoders: BTreeMap::new(),
        };
        registry
            .register_type::<External>()
            .expect("valid external job type");
        registry
    }
    pub fn register<T: Job>(&mut self) -> Result<()> {
        let patterns = T::patterns();
        let refs: Vec<_> = patterns.iter().map(String::as_str).collect();
        self.register_with::<T, _>(T::TYPE, &refs, T::parse)
    }
    /// Register a serializable internal/rewrite-only job without a source matcher.
    pub fn register_type<T: Job>(&mut self) -> Result<()> {
        if T::TYPE.is_empty() {
            bail!("job type tag must not be empty");
        }
        if let Some(existing) = self.decoders.get(T::TYPE) {
            if existing.rust_type != TypeId::of::<T>() {
                bail!(
                    "job type tag {} is already registered for {}, not {}",
                    T::TYPE,
                    existing.rust_name,
                    type_name::<T>()
                );
            }
            return Ok(());
        }
        self.decoders.insert(
            T::TYPE,
            Decoder {
                rust_type: TypeId::of::<T>(),
                rust_name: type_name::<T>(),
                version: T::VERSION,
                decode: JobRecord::restore::<T>,
            },
        );
        Ok(())
    }
    pub fn register_with<T: Job, F>(
        &mut self,
        classification: &str,
        patterns: &[&str],
        parse: F,
    ) -> Result<()>
    where
        F: Fn(&ShellCommand) -> Result<T> + Send + Sync + 'static,
    {
        let patterns = compile_patterns(classification, patterns)?;
        self.register_type::<T>()?;
        self.handlers.push(Handler {
            classification: classification.into(),
            rust_type: TypeId::of::<T>(),
            patterns,
            parse: Box::new(move |source, shell| {
                let typed = parse(&shell).with_context(|| {
                    format!(
                        "{} parser failed on line {}: {}",
                        T::TYPE,
                        source.line,
                        source.text
                    )
                })?;
                JobRecord::new(source, Some(shell), typed)
            }),
        });
        Ok(())
    }
    /// Replace only this concrete type's classification rules. Other tool roles
    /// retain their matchers, and overlaps still fail instead of taking priority.
    pub fn replace_patterns<T: Job>(&mut self, patterns: &[&str]) -> Result<()> {
        let patterns = compile_patterns(T::TYPE, patterns)?;
        self.register_type::<T>()?;
        self.handlers
            .retain(|handler| handler.rust_type != TypeId::of::<T>());
        self.handlers.push(Handler {
            classification: T::TYPE.into(),
            rust_type: TypeId::of::<T>(),
            patterns,
            parse: Box::new(|source, shell| {
                let typed = T::parse(&shell).with_context(|| {
                    format!(
                        "{} parser failed on line {}: {}",
                        T::TYPE,
                        source.line,
                        source.text
                    )
                })?;
                JobRecord::new(source, Some(shell), typed)
            }),
        });
        Ok(())
    }
    pub fn register_external(&mut self, classification: &str, patterns: &[&str]) -> Result<()> {
        let identity = classification.to_owned();
        self.register_with::<External, _>(classification, patterns, move |shell| {
            Ok(External::from_shell(identity.clone(), shell))
        })
    }
    pub fn add_python(&mut self, identity: &str, script: &str) -> Result<()> {
        if identity.is_empty() || script.is_empty() {
            bail!("Python configuration requires an identity and script");
        }
        let interpreter = r"(?:python(?:[0-9]+(?:\.[0-9]+)*)?|py)";
        let prefix = command_pattern(interpreter);
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
        let pattern = format!(r"{prefix}\s+(?:{})(?:\s|$|[<>;&|])", spellings.join("|"));
        let configured_identity = identity.to_owned();
        let script = script.to_owned();
        self.register_with::<Python, _>(identity, &[&pattern], move |shell| {
            Python::identified(&configured_identity, &script, shell)
        })
    }
    pub fn classify(&self, source: SourceCommand) -> Result<JobRecord> {
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
                .map(|&(handler, pattern)| {
                    format!(
                        "  {}[{}]: {}",
                        self.handlers[handler].classification,
                        pattern,
                        self.handlers[handler].patterns[pattern]
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
        let Some(&(handler_index, _)) = matches.first() else {
            println!("{UNKNOWN_PREFIX}{}", source.text);
            let job = shell.as_ref().map_or_else(
                || External::opaque("unknown"),
                |command| External::from_shell("unknown", command),
            );
            return JobRecord::new(source, shell, job);
        };
        let handler = &self.handlers[handler_index];
        match shell {
            Some(shell) => (handler.parse)(source, shell),
            None => JobRecord::new(
                source,
                None,
                External::opaque(handler.classification.clone()),
            ),
        }
    }
    pub fn restore(&self, snapshot: JobSnapshot) -> Result<JobRecord> {
        let decoder = self.decoders.get(snapshot.spec.type_tag.as_str())
            .with_context(|| format!("job type {} is not registered; register its implementation before loading this plan", snapshot.spec.type_tag))?;
        if decoder.version != snapshot.spec.handler_version {
            bail!(
                "job type {} version {} is incompatible with registered version {}",
                snapshot.spec.type_tag,
                snapshot.spec.handler_version,
                decoder.version
            );
        }
        (decoder.decode)(snapshot)
    }
}
fn compile_patterns(classification: &str, patterns: &[&str]) -> Result<Vec<Regex>> {
    if classification.is_empty() || patterns.is_empty() {
        bail!(
            "registration requires a classification and at least one pattern; use register_type for internal jobs"
        );
    }
    patterns
        .iter()
        .map(|pattern| {
            Regex::new(pattern)
                .with_context(|| format!("invalid regex for handler {classification}: {pattern}"))
        })
        .collect()
}
pub fn command_pattern(programs: &str) -> String {
    format!(
        r#"^\s*(?:(?:[^\s'\";|&<>]*/)?(?:{programs})|'(?:[^']*/)?(?:{programs})'|\"(?:[^\"]*/)?(?:{programs})\")(?:\s|$|[<>;&|])"#
    )
}
pub fn binary_pattern(programs: &str) -> String {
    command_pattern(&format!(
        r"(?:[A-Za-z0-9_+.]+-)*(?:{programs})(?:-[0-9]+(?:\.[0-9]+)*)?(?:\.exe)?"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::cp::Cp;
    fn source(text: &str) -> SourceCommand {
        SourceCommand {
            line: 1,
            text: text.into(),
            target: None,
        }
    }
    #[test]
    fn parses_typed_jobs_and_keeps_registered_errors_fatal() {
        let registry = Registry::default();
        let record = registry.classify(source("cp a b")).unwrap();
        assert!(record.typed::<Cp>().is_some());
        assert!(record.spec.cacheable);
        assert!(registry.classify(source("cp -r a b")).is_err());
        assert!(
            registry
                .classify(source("unregistered 'unterminated"))
                .is_err()
        );
    }
    #[test]
    fn every_pattern_counts_even_in_one_registration() {
        let mut registry = Registry::default();
        registry
            .register_external("custom", &["^widget", "widget"])
            .unwrap();
        let error = registry
            .classify(source("widget x"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("ambiguous command classification") && error.contains("widget x"));
        assert!(error.contains(
            "2 matching patterns\nMatching patterns:\n  custom[0]: ^widget\n  custom[1]: widget"
        ));
    }
    #[test]
    fn redirections_switch_native_strategy_but_preserve_tool_identity() {
        let registry = Registry::default();
        let record = registry.classify(source("cp a b > log")).unwrap();
        assert!(record.typed::<Cp>().is_some());
        assert!(record.spec.unknown_effects);
        assert!(!record.spec.cacheable);
        assert_eq!(record.strategy, crate::job::ExecutionStrategy::External);
        assert_eq!(
            registry
                .classify(source("cp a b && touch done"))
                .unwrap()
                .spec
                .kind,
            "cp"
        );
    }
    #[test]
    fn python_identities_and_overlapping_patterns_remain_distinct() {
        let mut registry = Registry::default();
        registry
            .add_python("generate", "scripts/make data.py")
            .unwrap();
        for text in [
            "python 'scripts/make data.py' --out result",
            "/usr/bin/python3 \"scripts/make data.py\"",
            "py 'scripts/make data.py'",
        ] {
            let record = registry.classify(source(text)).unwrap();
            assert_eq!(record.spec.kind, "generate");
            assert!(record.spec.unknown_effects);
        }
        assert_eq!(
            registry
                .classify(source("python other.py"))
                .unwrap()
                .spec
                .kind,
            "unknown"
        );
        registry
            .add_python("overlap", "scripts/make data.py")
            .unwrap();
        assert!(
            registry
                .classify(source("python 'scripts/make data.py'"))
                .unwrap_err()
                .to_string()
                .contains("overlap[0]")
        );
    }
    #[test]
    fn invalid_patterns_do_not_leave_partial_registrations() {
        let mut registry = Registry::default();
        assert!(
            registry
                .register_external("bad", &["^widget", "["])
                .is_err()
        );
        assert_eq!(
            registry.classify(source("widget x")).unwrap().spec.kind,
            "unknown"
        );
    }
}
