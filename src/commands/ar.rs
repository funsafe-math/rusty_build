use anyhow::Result;

use super::{binary_common, external::execute_argv};
use crate::{
    job::{Job, JobData},
    shell::ShellHints,
};

use super::binary_common::{Effects, command, files, known, parse, paths};
use clap::Arg;

fn effects(argv: &[String]) -> std::result::Result<Effects, String> {
    // Traditional ar's first word is an operation plus modifiers, optionally
    // prefixed by one dash. Only that positional word admits a leading dash.
    let matches = parse(
        command("ar")
            .arg(
                Arg::new("operation")
                    .required(true)
                    .allow_hyphen_values(true),
            )
            .arg(files(2, None)),
        argv,
    )?;
    let operation = matches
        .get_one::<String>("operation")
        .expect("required operation");
    let operation = operation.strip_prefix('-').unwrap_or(operation);
    let mut replace = false;
    for modifier in operation.chars() {
        match modifier {
            'r' if !replace => replace = true,
            'c' | 's' | 'D' => (),
            _ => return Err("only ar r with c/s/D modifiers has understood file effects; thin, extract, plugin, script and other modes remain opaque".into()),
        }
    }
    if !replace {
        return Err("only archive replacement (ar r) is understood".into());
    }
    let inputs = paths(&matches)?;
    // The existing archive is read as well as rewritten. Member files supply
    // replacements, and insertion creates no files outside the archive itself.
    let outputs = vec![inputs[0].clone()];
    Ok(known(inputs, outputs))
}

#[derive(Debug)]
pub struct Ar {
    pub data: JobData,
    pub argv: Vec<String>,
}

impl Ar {
    pub fn parse(argv: &[String], _hints: &ShellHints) -> Result<Self> {
        Ok(Self {
            data: binary_common::job_data(effects(argv)),
            argv: argv.to_vec(),
        })
    }
}

impl Job for Ar {
    fn data(&self) -> &JobData {
        &self.data
    }
    fn data_mut(&mut self) -> &mut JobData {
        &mut self.data
    }
    fn execute(&self) -> Result<()> {
        execute_argv(&self.argv, &self.data.shell)
    }

    fn check_inputs(&self, inputs: &[&str]) -> Result<()> {
        if self.data.shell.ignore_failure {
            return Ok(());
        }
        // Replacement reads an existing archive but is also allowed to create it.
        // Keep ReadWrite for graph ordering without requiring it to preexist.
        for path in inputs {
            let archive = self.data.accesses.iter().any(|access| {
                access.path == *path && access.access == crate::job::Access::ReadWrite
            });
            if archive
                && std::fs::metadata(path)
                    .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            {
                continue;
            }
            crate::job::check_inputs(std::slice::from_ref(path))?;
        }
        Ok(())
    }
}
