use anyhow::Result;
use clap::Parser;

use crate::job::{FileAccess, Job, JobData};
use crate::shell::ShellHints;

use super::external::execute_argv;

const SCRIPT: &str = "tools/generate_header.py";

/// Effect contract for the trusted fixture generator, including its script read.
/// Registering this handler assumes the named script has the fixture's semantics.
#[derive(Debug)]
pub struct GenerateHeadersPy {
    pub data: JobData,
    pub argv: Vec<String>,
    pub script: String,
    pub template: String,
    pub output: String,
}

#[derive(Parser)]
#[command(disable_help_flag = true, disable_version_flag = true)]
struct GenerateHeadersArgs {
    script: String,
    template: String,
    #[arg(long)]
    output: String,
}

fn is_generator(script: &str) -> bool {
    script == SCRIPT
        || (script.starts_with('/') && script.ends_with("/tools/generate_header.py"))
        || (cfg!(windows)
            && script.as_bytes().get(1) == Some(&b':')
            && (script.ends_with("/tools/generate_header.py")
                || script.ends_with("\\tools\\generate_header.py")))
}

impl GenerateHeadersPy {
    /// Claim only the known script form; interpreter flags and other scripts
    /// belong to other nested handlers rather than a generic Python parser.
    pub fn parse(argv: &[String], _hints: &ShellHints) -> Result<Option<Box<dyn Job>>> {
        if !argv.get(1).is_some_and(|script| is_generator(script)) {
            return Ok(None);
        }
        let args = GenerateHeadersArgs::try_parse_from(argv)?;
        let data = JobData::new(vec![
            FileAccess::read(&args.script),
            FileAccess::read(&args.template),
            FileAccess::write(&args.output),
        ]);
        Ok(Some(Box::new(Self {
            data,
            argv: argv.to_vec(),
            script: args.script,
            template: args.template,
            output: args.output,
        })))
    }
}

impl Job for GenerateHeadersPy {
    fn data(&self) -> &JobData {
        &self.data
    }
    fn data_mut(&mut self) -> &mut JobData {
        &mut self.data
    }
    fn execute(&self) -> Result<()> {
        execute_argv(&self.argv, &self.data.shell)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{any::Any, fs};

    #[test]
    fn claims_only_its_script_and_validates_its_own_grammar() {
        let hints = ShellHints::default();
        for argv in [
            vec!["python3", "other.py", "input", "--output", "result"],
            vec!["python3", "-O", SCRIPT, "input", "--output", "result"],
            vec![
                "python3",
                "somewhere/generate_header.py",
                "input",
                "--output",
                "result",
            ],
            vec!["python3", "-c", "print('not a script invocation')"],
        ] {
            assert!(
                GenerateHeadersPy::parse(
                    &argv.into_iter().map(str::to_owned).collect::<Vec<_>>(),
                    &hints
                )
                .unwrap()
                .is_none()
            );
        }
        for argv in [
            vec!["python3", SCRIPT, "input"],
            vec![
                "python3",
                SCRIPT,
                "input",
                "--output",
                "result",
                "--unknown",
            ],
        ] {
            assert!(
                GenerateHeadersPy::parse(
                    &argv.into_iter().map(str::to_owned).collect::<Vec<_>>(),
                    &hints
                )
                .is_err()
            );
        }
    }

    #[test]
    fn declares_exact_script_template_and_header_paths() {
        let argv = [
            "python3",
            SCRIPT,
            "templates/../input.h.in",
            "--output",
            "generated/output.h",
        ]
        .map(str::to_owned);
        let job = GenerateHeadersPy::parse(&argv, &ShellHints::default())
            .unwrap()
            .unwrap();
        let generator = (job.as_ref() as &dyn Any)
            .downcast_ref::<GenerateHeadersPy>()
            .unwrap();
        assert_eq!(generator.argv, argv);
        assert_eq!(generator.template, "templates/../input.h.in");
        assert_eq!(
            job.data().accesses,
            vec![
                FileAccess::read(SCRIPT),
                FileAccess::read("templates/../input.h.in"),
                FileAccess::write("generated/output.h"),
            ]
        );
        assert!(!job.data().unknown_effects);
    }

    #[test]
    fn runs_the_actual_script_with_absolute_fixture_paths_and_redirects() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("tools")).unwrap();
        let script = root.path().join(SCRIPT);
        fs::write(
            &script,
            include_str!("../../fixtures/python_headers/tools/generate_header.py"),
        )
        .unwrap();
        let template = root.path().join("input.h.in");
        fs::write(&template, "#define ANSWER @ANSWER@\n").unwrap();
        let output = root.path().join("generated/output.h");
        let argv = vec![
            "python3".to_owned(),
            script.to_str().unwrap().into(),
            template.to_str().unwrap().into(),
            "--output".into(),
            output.to_str().unwrap().into(),
        ];
        fs::create_dir(root.path().join("generated")).unwrap();
        let mut hints = ShellHints::default();
        let log = root.path().join("generator.log");
        hints.stdout = Some(log.to_str().unwrap().into());
        let mut job = GenerateHeadersPy::parse(&argv, &hints).unwrap().unwrap();
        job.data_mut().shell = hints;
        job.check_inputs(&[script.to_str().unwrap(), template.to_str().unwrap()])
            .unwrap();
        job.execute().unwrap();
        assert_eq!(fs::read_to_string(output).unwrap(), "#define ANSWER 42\n");
        assert!(log.is_file());
    }
}
