use crate::{
    job::Job,
    shell::{ParsedLine, ShellHints},
};
use anyhow::{Context, Result, bail, ensure};

type Handler = dyn Fn(&[String], &ShellHints) -> Result<Option<Box<dyn Job>>>;

struct Registration {
    names: Vec<String>,
    parse: Box<Handler>,
}

pub struct Registry {
    handlers: Vec<Registration>,
}

impl Default for Registry {
    fn default() -> Self {
        let mut registry = Self::empty();
        crate::commands::register_builtins(&mut registry).expect("valid built-in registrations");
        registry
    }
}

impl Registry {
    pub fn empty() -> Self {
        Self {
            handlers: Vec::new(),
        }
    }

    pub fn register(
        &mut self,
        names: &[&str],
        parse: impl Fn(&[String], &ShellHints) -> Result<Option<Box<dyn Job>>> + 'static,
    ) -> Result<()> {
        ensure!(!names.is_empty(), "a handler needs an executable name");
        for (index, name) in names.iter().enumerate() {
            ensure!(
                !name.is_empty() && !name.contains('/'),
                "register executable basenames, not paths"
            );
            if names[..index].contains(name)
                || self
                    .handlers
                    .iter()
                    .any(|handler| handler.names.iter().any(|existing| existing == name))
            {
                bail!("executable handler already registered: {name}");
            }
        }
        self.handlers.push(Registration {
            names: names.iter().map(|name| (*name).to_owned()).collect(),
            parse: Box::new(parse),
        });
        Ok(())
    }

    pub fn register_external(&mut self, names: &[&str]) -> Result<()> {
        self.register(names, |argv, shell| {
            Ok(Some(Box::new(crate::commands::external::External::parse(
                argv, shell,
            )?)))
        })
    }

    pub fn parse(&self, command: ParsedLine) -> Result<Box<dyn Job>> {
        let executable = executable_name(&command.argv[0]);
        let registration = self
            .handlers
            .iter()
            .find(|handler| handler.names.iter().any(|name| name == executable))
            .with_context(|| {
                format!(
                    "no handler for executable {executable:?} on line {}",
                    command.line
                )
            })?;
        let mut job = (registration.parse)(&command.argv, &command.shell)
            .with_context(|| format!("parse line {}: {}", command.line, command.text))?
            .with_context(|| {
                format!(
                    "no handler accepts invocation on line {}: {}",
                    command.line, command.text
                )
            })?;
        let data = job.data_mut();
        data.line = command.line;
        data.text = command.text;
        data.set_shell(command.shell);
        Ok(job)
    }
}

pub fn executable_name(program: &str) -> &str {
    let name = program.rsplit('/').next().unwrap_or(program);
    name.strip_suffix(".exe").unwrap_or(name)
}
