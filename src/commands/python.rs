use anyhow::Result;

use crate::job::Job;
use crate::shell::ShellHints;

type Handler = Box<dyn Fn(&[String], &ShellHints) -> Result<Option<Box<dyn Job>>>>;

/// Nested, explicitly registered script handlers, not a Python command grammar.
#[derive(Default)]
pub struct Python {
    handlers: Vec<Handler>,
}

impl Python {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        handler: impl Fn(&[String], &ShellHints) -> Result<Option<Box<dyn Job>>> + 'static,
    ) {
        self.handlers.push(Box::new(handler));
    }

    /// The first claiming handler wins; errors from a claiming parser propagate.
    /// Every handler receives the complete original argv and shell hints.
    pub fn parse(&self, argv: &[String], hints: &ShellHints) -> Result<Option<Box<dyn Job>>> {
        for handler in &self.handlers {
            if let Some(job) = handler(argv, hints)? {
                return Ok(Some(job));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::JobData;
    use std::{cell::RefCell, rc::Rc};

    #[derive(Debug, Default)]
    struct ScriptJob {
        data: JobData,
    }

    impl Job for ScriptJob {
        fn data(&self) -> &JobData {
            &self.data
        }
        fn data_mut(&mut self) -> &mut JobData {
            &mut self.data
        }
        fn execute(&self) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn nested_dispatch_passes_full_argv_and_hints_without_interpreting_python() {
        let argv = [
            "custom-python",
            "-I",
            "-m",
            "package.generator",
            "--output",
            "file.h",
        ]
        .map(str::to_owned);
        let mut hints = ShellHints::default();
        hints.stdout = Some("log".into());
        let calls = Rc::new(RefCell::new(Vec::new()));
        let mut inner = Python::new();
        let seen = Rc::clone(&calls);
        inner.register(move |argv, hints| {
            seen.borrow_mut().push((argv.to_vec(), hints.clone()));
            Ok(Some(Box::new(ScriptJob::default())))
        });
        let mut outer = Python::new();
        let declined = Rc::clone(&calls);
        outer.register(move |argv, hints| {
            declined.borrow_mut().push((argv.to_vec(), hints.clone()));
            Ok(None)
        });
        outer.register(move |argv, hints| inner.parse(argv, hints));
        outer.register(|_, _| panic!("must stop after a handler claims the invocation"));
        assert!(outer.parse(&argv, &hints).unwrap().is_some());
        assert_eq!(
            *calls.borrow(),
            vec![(argv.to_vec(), hints.clone()), (argv.to_vec(), hints)]
        );
    }

    #[test]
    fn unrelated_scripts_are_declined_and_errors_are_not_swallowed() {
        let argv = ["python3", "unrelated.py"].map(str::to_owned);
        let hints = ShellHints::default();
        assert!(Python::new().parse(&argv, &hints).unwrap().is_none());
        let mut python = Python::new();
        python.register(super::super::generate_headers_py::GenerateHeadersPy::parse);
        assert!(python.parse(&argv, &hints).unwrap().is_none());
        let malformed = ["python3", "tools/generate_header.py", "input"].map(str::to_owned);
        assert!(python.parse(&malformed, &hints).is_err());
    }
}
