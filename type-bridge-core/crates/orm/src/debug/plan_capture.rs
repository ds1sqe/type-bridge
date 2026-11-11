//! Internal conformance sink, compiled out of ordinary builds and host logs.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::query_plan::QueryPlan;

struct Sink {
    output: PathBuf,
    label: PathBuf,
    lock: Mutex<()>,
}

pub(crate) fn record(plan: &QueryPlan) {
    static SINK: OnceLock<Option<Sink>> = OnceLock::new();
    let sink = SINK.get_or_init(|| {
        std::env::var_os("TYPE_BRIDGE_TEST_PLAN_CAPTURE").map(|output| Sink {
            output: output.into(),
            label: std::env::var_os("TYPE_BRIDGE_TEST_PLAN_CAPTURE_LABEL")
                .expect("the internal plan capture needs its label file")
                .into(),
            lock: Mutex::new(()),
        })
    });
    if let Some(sink) = sink {
        let _lock = sink.lock.lock().expect("internal plan capture lock");
        append(&sink.output, &sink.label, &[plan])
            .expect("the internal plan capture must retain every selected stack");
    }
}

fn append(output: &Path, label: &Path, plans: &[&QueryPlan]) -> std::io::Result<()> {
    let label = fs::read_to_string(label)?;
    let label = label.trim();
    if label.is_empty() {
        return Ok(());
    }
    if label.len() > 160
        || !label.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
    {
        return Err(std::io::Error::other("invalid internal plan capture label"));
    }
    let plans = plans
        .iter()
        .map(|plan| {
            let bytes = plan.canonical_bytes().map_err(std::io::Error::other)?;
            serde_json::from_slice::<serde_json::Value>(&bytes).map_err(std::io::Error::other)
        })
        .collect::<std::io::Result<Vec<_>>>()?;
    let value = serde_json::json!({ "call": label, "plans": plans });
    let mut bytes = to_canonical_json(&value).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(output)?
        .write_all(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_labels_record_nothing_and_selected_calls_keep_their_order() {
        let directory = std::env::temp_dir().join(format!(
            "typebridge-plan-capture-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir(&directory).unwrap();
        let output = directory.join("stacks.jsonl");
        let label = directory.join("label");
        fs::write(&label, "").unwrap();
        append(&output, &label, &[]).unwrap();
        assert!(!output.exists());
        fs::write(&label, "manager_field_token_filter.count").unwrap();
        append(&output, &label, &[]).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&fs::read(&output).unwrap()).unwrap();
        assert_eq!(
            value,
            serde_json::json!({ "call":"manager_field_token_filter.count", "plans":[] })
        );
        fs::write(&label, "manager_field_token_filter.first").unwrap();
        append(&output, &label, &[]).unwrap();
        let frames = fs::read_to_string(&output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0]["call"], "manager_field_token_filter.count");
        assert_eq!(frames[1]["call"], "manager_field_token_filter.first");
        fs::write(&label, "invalid\nlabel").unwrap();
        assert!(append(&output, &label, &[]).is_err());
        fs::remove_dir_all(directory).unwrap();
    }
}
