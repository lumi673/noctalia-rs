use noctalia_rs::compat::Parts;
use noctalia_rs::{Context, Op, State, Value};
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(State)]
#[state(key = "tests.counter")]
struct Counter {
    #[state(default = 2)]
    count: i64,
    #[state(default = false)]
    enabled: bool,
    label: Option<String>,
    #[state(skip)]
    cache: i64,
}

fn context() -> Context {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let data_dir = PathBuf::from(format!("/tmp/noctalia-state-derive-{nanos}"));
    let _ = fs::create_dir_all(&data_dir);
    let mut ctx = Context::new(
        Parts {
            entry: "entry".to_owned(),
            event: "update".to_owned(),
            payload: Value::Null,
            config: Value::Null,
            state: Value::Null,
            host: json!({}),
            data_dir,
        },
        Value::Null,
    );
    ctx.set_op_sink(Arc::new(|op| {
        let Ok(value) = serde_json::to_value(op) else {
            return;
        };
        let Some(path) = value.get("response").and_then(Value::as_str) else {
            return;
        };
        let response = json!({
            "ok": true,
            "value": { "count": 9, "cache": 99 },
            "error": Value::Null,
        });
        if let Ok(text) = serde_json::to_string(&response) {
            let _ = fs::write(path, text);
        }
    }));
    ctx
}

#[test]
fn derive_builds_a_default_table_without_null_slots() {
    assert_eq!(Counter::defaults(), json!({ "count": 2, "enabled": false }));
    assert_eq!(Counter::KEY, "tests.counter");
}

#[test]
fn derive_load_merges_host_state_over_defaults() {
    let state = Counter::load(&context());
    assert_eq!(state.count, 9);
    assert!(!state.enabled);
    assert_eq!(state.label, None);
    assert_eq!(state.cache, 0);
}

#[test]
fn derive_save_and_watch_use_the_declared_key() {
    let state = Counter {
        count: 4,
        enabled: true,
        label: None,
        cache: 99,
    };
    let mut ops = Vec::new();
    state.save(&mut ops);
    Counter::watch(&mut ops, "changed");
    let values = ops
        .iter()
        .map(|op| serde_json::to_value(op).expect("serialize op"))
        .collect::<Vec<_>>();
    assert_eq!(values[0]["op"], json!("state_set"));
    assert_eq!(values[0]["key"], json!("tests.counter"));
    assert_eq!(values[0]["value"], json!({ "count": 4, "enabled": true }));
    assert_eq!(values[1]["path"], json!(["state", "watch"]));
    assert_eq!(values[1]["args"][0], json!("tests.counter"));
    assert_eq!(
        values[1]["args"][1],
        json!({ "__host_callback": "changed" })
    );
}

#[test]
fn derive_from_value_replaces_wrong_types_with_defaults() {
    let state =
        Counter::from_value(&json!({ "count": "bad", "enabled": true }));
    assert_eq!(state.count, 2);
    assert!(state.enabled);
    assert_eq!(state.cache, 0);
    assert_eq!(state.to_value(), json!({ "count": 2, "enabled": true }));
}

#[test]
fn ops_still_construct_directly() {
    let op: Op = Op::state("tests.counter", json!({ "count": 1 }));
    assert_eq!(
        serde_json::to_value(op).expect("serialize")["op"],
        json!("state_set")
    );
}

#[derive(State)]
#[state(key = "tests.maybe")]
struct Maybe {
    #[state(default = Some(5))]
    count: Option<i64>,
}

#[test]
fn derive_accepts_some_defaults() {
    assert_eq!(Maybe::defaults(), json!({ "count": 5 }));
    let value = Maybe::from_value(&json!({ "count": 7 }));
    assert_eq!(value.count, Some(7));
}
