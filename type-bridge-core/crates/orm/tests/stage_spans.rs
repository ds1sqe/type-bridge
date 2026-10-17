//! A staged commit names its ORM stages with field-free `tracing` spans,
//! which the perf harness aggregates per stage.

#[path = "../benches/common/mod.rs"]
mod common;

use std::sync::{Arc, Mutex};

use tracing::span::{Attributes, Id};
use tracing::{Subscriber, subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

/// Records the name and field count of every span opened.
struct Spans(Arc<Mutex<Vec<(&'static str, usize)>>>);

impl<S: Subscriber> Layer<S> for Spans {
    fn on_new_span(&self, attributes: &Attributes<'_>, _: &Id, _: Context<'_, S>) {
        let metadata = attributes.metadata();
        self.0
            .lock()
            .unwrap()
            .push((metadata.name(), metadata.fields().len()));
    }
}

#[test]
fn a_staged_commit_opens_one_plan_and_one_compile_span_per_plan() {
    let staged = common::staged_commit();
    let spans = Arc::new(Mutex::new(Vec::new()));
    let recorder = tracing_subscriber::registry().with(Spans(Arc::clone(&spans)));
    let answers = subscriber::with_default(recorder, || staged.run());
    assert_eq!(answers, 3);

    let spans = spans.lock().unwrap();
    let stages: Vec<_> = spans
        .iter()
        .filter(|(name, _)| name.starts_with("tb."))
        .collect();
    let count = |stage: &str| stages.iter().filter(|(name, _)| *name == stage).count();
    assert_eq!(count("tb.orm.plan"), 3, "{stages:?}");
    assert_eq!(count("tb.query.compile"), 3, "{stages:?}");
    // No stage span carries a field, so no query text or value is recorded.
    assert!(stages.iter().all(|(_, fields)| *fields == 0), "{stages:?}");
}
