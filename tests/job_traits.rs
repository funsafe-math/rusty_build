use anyhow::{Result, bail, ensure};
use rusty_make::{
    create_graph, execute,
    job::{FileAccess, Job, JobData},
    parse,
    registry::Registry,
};
use std::{any::Any, fs};

#[derive(Debug)]
struct WriteJob {
    data: JobData,
    output: String,
    payload: String,
    fail: bool,
}
impl Job for WriteJob {
    fn data(&self) -> &JobData {
        &self.data
    }
    fn data_mut(&mut self) -> &mut JobData {
        &mut self.data
    }
    fn execute(&self) -> Result<()> {
        fs::write(&self.output, &self.payload)?;
        if self.fail {
            bail!("requested failure after partial output");
        }
        Ok(())
    }
}

fn writer(output: String, payload: &str, fail: bool) -> Box<dyn Job> {
    Box::new(WriteJob {
        data: JobData::new(vec![FileAccess::write(&output)]),
        output,
        payload: payload.into(),
        fail,
    })
}

#[test]
fn default_batch_propagates_failure_and_stops_remaining_work() {
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first").to_string_lossy().into_owned();
    let second = root.path().join("second").to_string_lossy().into_owned();
    let dag = create_graph(vec![
        writer(first, "partial", true),
        writer(second, "must not run", false),
    ])
    .unwrap();
    assert!(execute(&dag, false).is_err());
    assert_eq!(fs::read(root.path().join("first")).unwrap(), b"partial");
    assert!(!root.path().join("second").exists());
}

#[test]
fn custom_handler_and_typed_preprocessing_keep_the_original_job() {
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("output").to_string_lossy().into_owned();
    let mut registry = Registry::empty();
    registry
        .register(&["write-data"], |argv, _| {
            ensure!(argv.len() == 3, "write-data requires output and payload");
            Ok(Some(writer(argv[1].clone(), &argv[2], false)))
        })
        .unwrap();
    let text = format!("write-data {} initial\n", shell_words::quote(&output));
    let mut jobs = parse(&text, &registry).unwrap();
    let any: &mut dyn Any = jobs[0].as_mut();
    any.downcast_mut::<WriteJob>().unwrap().payload = "enriched".into();
    let dag = create_graph(jobs).unwrap();
    execute(&dag, false).unwrap();
    assert_eq!(fs::read(root.path().join("output")).unwrap(), b"enriched");
}
