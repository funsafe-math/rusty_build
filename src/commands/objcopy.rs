use anyhow::Result;

use super::{binary_common, external::execute_argv};
use crate::{
    job::{Job, JobData},
    shell::ShellHints,
};

use super::binary_common::{
    Effects, command, copy_options, files, flag, known, parse, path, paths, value,
};

fn effects(argv: &[String]) -> std::result::Result<Effects, String> {
    // objcopy uses -S for strip-all, unlike strip's -s. Keep the
    // shared option's long spelling, assigning that short spelling here.
    let mut parser = copy_options(command("objcopy"))
        .mut_arg("strip-all", |arg| arg.short('S'))
        .arg(flag("strip-debug", Some('g')))
        .arg(value("binary-architecture", Some('B')))
        .arg(value("only-section", Some('j')))
        .arg(value("add-section", None))
        .arg(value("update-section", None))
        .arg(value("dump-section", None))
        .arg(files(1, Some(2)));
    for name in [
        "set-section-flags",
        "set-section-alignment",
        "rename-section",
    ] {
        parser = parser.arg(value(name, None));
    }
    let matches = parse(parser, argv)?;
    let mut operands = paths(&matches)?;
    let output = if operands.len() == 2 {
        operands.pop().expect("two object operands")
    } else {
        operands[0].clone()
    };
    let mut result = known(operands, vec![output]);
    for option in ["add-section", "update-section", "dump-section"] {
        for specification in matches.get_many::<String>(option).into_iter().flatten() {
            let (section, filename) = specification
                .split_once('=')
                .ok_or_else(|| format!("--{option} requires SECTION=FILE"))?;
            if section.is_empty() {
                return Err(format!("--{option} requires a named section"));
            }
            let filename = path(filename)?;
            if option == "dump-section" {
                result.outputs.push(filename);
            } else {
                result.inputs.push(filename);
            }
        }
    }
    Ok(result)
}

#[derive(Debug)]
pub struct Objcopy {
    pub data: JobData,
    pub argv: Vec<String>,
}

impl Objcopy {
    pub fn parse(argv: &[String], _hints: &ShellHints) -> Result<Self> {
        Ok(Self {
            data: binary_common::job_data(effects(argv)),
            argv: argv.to_vec(),
        })
    }
}

impl Job for Objcopy {
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
