use rusty_make::{
    graph::{Edge, create_graph},
    job::{FileAccess, Job, JobData},
    scheduler::execute,
};
use std::{any::Any, cell::RefCell, fs, rc::Rc};

type Events = Rc<RefCell<Vec<String>>>;

#[derive(Debug)]
struct CopyFile {
    data: JobData,
    source: String,
    destination: String,
    events: Events,
}
impl Job for CopyFile {
    fn data(&self) -> &JobData {
        &self.data
    }
    fn data_mut(&mut self) -> &mut JobData {
        &mut self.data
    }
    fn execute(&self) -> anyhow::Result<()> {
        fs::copy(&self.source, &self.destination)?;
        self.events.borrow_mut().push(self.destination.clone());
        Ok(())
    }
}
fn copy(source: String, destination: String, events: &Events) -> Box<dyn Job> {
    Box::new(CopyFile {
        data: JobData::new(vec![
            FileAccess::read(source.clone()),
            FileAccess::write(destination.clone()),
        ]),
        source,
        destination,
        events: events.clone(),
    })
}

#[test]
fn default_batch_executes_each_item_then_dependent_consumer() {
    let root = tempfile::tempdir().unwrap();
    let path = |name| root.path().join(name).to_str().unwrap().to_owned();
    fs::write(path("a"), "A").unwrap();
    fs::write(path("b"), "B").unwrap();
    let events = Events::default();
    let dag = create_graph(vec![
        copy(path("a"), path("x"), &events),
        copy(path("b"), path("y"), &events),
        copy(path("x"), path("z"), &events),
    ])
    .unwrap();
    assert_eq!(dag.batches(), vec![vec![0, 1], vec![2]]);
    execute(&dag, false).unwrap();
    assert_eq!(*events.borrow(), vec![path("x"), path("y"), path("z")]);
    assert_eq!(fs::read_to_string(path("z")).unwrap(), "A");
    assert_eq!(fs::read_to_string(path("y")).unwrap(), "B");
}

#[derive(Debug)]
struct BatchItem {
    data: JobData,
    id: usize,
    events: Events,
}
impl Job for BatchItem {
    fn data(&self) -> &JobData {
        &self.data
    }
    fn data_mut(&mut self) -> &mut JobData {
        &mut self.data
    }
    fn name(&self) -> &str {
        "same-name"
    }
    fn execute(&self) -> anyhow::Result<()> {
        anyhow::bail!("custom batch must be called")
    }
    fn execute_batch(&self, items: &[&dyn Job]) -> anyhow::Result<()> {
        self.events.borrow_mut().push("batch:start".into());
        for item in items {
            let item = (*item as &dyn Any)
                .downcast_ref::<Self>()
                .expect("only same concrete type belongs in a batch");
            self.events.borrow_mut().push(format!("batch:{}", item.id));
        }
        self.events.borrow_mut().push("batch:end".into());
        Ok(())
    }
}
#[derive(Debug)]
struct OtherItem {
    data: JobData,
    id: usize,
    events: Events,
}
impl Job for OtherItem {
    fn data(&self) -> &JobData {
        &self.data
    }
    fn data_mut(&mut self) -> &mut JobData {
        &mut self.data
    }
    fn name(&self) -> &str {
        "same-name"
    }
    fn execute(&self) -> anyhow::Result<()> {
        self.events.borrow_mut().push(format!("other:{}", self.id));
        Ok(())
    }
}

#[test]
fn custom_batches_are_sequential_stable_and_grouped_by_concrete_any_type() {
    let events = Events::default();
    let dag = create_graph(vec![
        Box::new(BatchItem {
            data: JobData::new(vec![FileAccess::write("token")]),
            id: 0,
            events: events.clone(),
        }) as Box<dyn Job>,
        Box::new(OtherItem {
            data: JobData::default(),
            id: 1,
            events: events.clone(),
        }),
        Box::new(BatchItem {
            data: JobData::default(),
            id: 2,
            events: events.clone(),
        }),
        Box::new(OtherItem {
            data: JobData::default(),
            id: 3,
            events: events.clone(),
        }),
        Box::new(BatchItem {
            data: JobData::new(vec![FileAccess::read("token")]),
            id: 4,
            events: events.clone(),
        }),
    ])
    .unwrap();
    assert_eq!(dag.batches(), vec![vec![0, 2], vec![1, 3], vec![4]]);
    execute(&dag, false).unwrap();
    assert_eq!(
        *events.borrow(),
        vec![
            "batch:start",
            "batch:0",
            "batch:2",
            "batch:end",
            "other:1",
            "other:3",
            "batch:start",
            "batch:4",
            "batch:end"
        ]
    );
}

#[derive(Debug)]
struct FailingItem {
    data: JobData,
    id: usize,
    fail: bool,
    events: Events,
}
impl Job for FailingItem {
    fn data(&self) -> &JobData {
        &self.data
    }
    fn data_mut(&mut self) -> &mut JobData {
        &mut self.data
    }
    fn execute(&self) -> anyhow::Result<()> {
        self.events.borrow_mut().push(format!("item:{}", self.id));
        if self.fail {
            anyhow::bail!("deliberate failure");
        }
        Ok(())
    }
}

#[test]
fn failure_stops_remaining_batch_items_and_all_later_batches() {
    let events = Events::default();
    let dag = create_graph(vec![
        Box::new(FailingItem {
            data: JobData::default(),
            id: 0,
            fail: true,
            events: events.clone(),
        }) as Box<dyn Job>,
        Box::new(FailingItem {
            data: JobData::default(),
            id: 1,
            fail: false,
            events: events.clone(),
        }),
        Box::new(OtherItem {
            data: JobData::default(),
            id: 2,
            events: events.clone(),
        }),
    ])
    .unwrap();
    assert!(execute(&dag, false).is_err());
    assert_eq!(*events.borrow(), vec!["item:0"]);
}

#[test]
fn early_input_failure_prevents_even_independent_side_effects() {
    let root = tempfile::tempdir().unwrap();
    let events = Events::default();
    let missing = root.path().join("missing").to_str().unwrap().to_owned();
    let dag = create_graph(vec![
        Box::new(OtherItem {
            data: JobData::default(),
            id: 0,
            events: events.clone(),
        }) as Box<dyn Job>,
        copy(
            missing,
            root.path().join("out").to_str().unwrap().to_owned(),
            &events,
        ),
    ])
    .unwrap();
    assert!(execute(&dag, false).is_err());
    assert!(events.borrow().is_empty());
    assert!(!root.path().join("out").exists());
}

#[test]
fn scheduler_revalidates_public_edges_before_input_checks_or_execution() {
    let events = Events::default();
    let mut dag = create_graph(vec![Box::new(OtherItem {
        data: JobData::default(),
        id: 0,
        events: events.clone(),
    }) as Box<dyn Job>])
    .unwrap();
    dag.edges.push(Edge { from: 0, to: 0 });
    assert!(
        execute(&dag, false)
            .unwrap_err()
            .to_string()
            .contains("cycle")
    );
    assert!(events.borrow().is_empty());
}
