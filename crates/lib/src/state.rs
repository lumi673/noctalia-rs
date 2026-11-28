//! Typed plugin state with build-time defaults and host persistence.
//!
//! A `#[derive(State)]` struct supplies the schema. `load` reads the host
//! state key and fills missing fields from `defaults`, `save` writes the whole
//! table back, and `watch` registers a host callback for the same key. Strict
//! builds lower these calls to direct plugin API 57 calls. Auto and off keep
//! the same behavior through the bridge.

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, to_value};

use crate::{Context, Op};

/// A named state table with typed fields and a default value per field.
pub trait State: Sized {
    /// Host state key this table is stored under.
    const KEY: &'static str;

    /// The table written when the host has no state for [`State::KEY`].
    fn defaults() -> Value;

    /// Reads a typed table, replacing missing or wrong-typed fields with
    /// their defaults.
    fn from_value(value: &Value) -> Self;

    /// Writes a typed table as JSON. `None` fields are omitted.
    fn to_value(&self) -> Value;
}

/// Reads a typed state table, merging the host value over the defaults.
///
/// A `None` field in the defaults is absent, so a stored `null` cannot reach
/// an array slot or a non-optional field.
pub fn load<T: State>(ctx: &Context) -> T {
    let mut value = T::defaults();
    if let (Some(defaults), Some(stored)) =
        (value.as_object_mut(), ctx.global_state(T::KEY))
        && let Some(object) = stored.as_object()
    {
        merge_stored(defaults, object);
    }
    T::from_value(&value)
}

fn merge_stored(
    defaults: &mut Map<String, Value>,
    stored: &Map<String, Value>,
) {
    for (key, item) in stored {
        if matches!(item, Value::Null) {
            continue;
        }
        defaults.insert(key.clone(), item.clone());
    }
}

/// Writes a typed state table to the host state service.
pub fn save<T: State>(state: &T, ops: &mut Vec<Op>) {
    ops.push(Op::state(T::KEY, state.to_value()));
}

/// Watches the state key of `T` and invokes `callback_id` on every change.
pub fn watch<T: State>(ops: &mut Vec<Op>, callback_id: &str) {
    ops.push(Op::state_watch(T::KEY, callback_id));
}

/// Starts a typed state object without a null slot. Used by the derive.
#[must_use]
pub fn object() -> Map<String, Value> {
    Map::new()
}

/// Decodes one field with its default when the stored value is missing or of
/// the wrong type. Used by the derive; public so generated code needs no
/// private imports.
pub fn field<T>(value: Option<&Value>, default: impl FnOnce() -> T) -> T
where
    T: DeserializeOwned,
{
    value
        .and_then(|item| serde_json::from_value(item.clone()).ok())
        .unwrap_or_else(default)
}

/// Inserts a field when present, omitting `None` so no JSON null crosses the
/// host boundary. Used by the derive.
pub fn insert<T: Serialize>(
    object: &mut Map<String, Value>,
    key: &str,
    value: &T,
) {
    if let Ok(encoded) = to_value(value)
        && !encoded.is_null()
    {
        object.insert(key.to_owned(), encoded);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compat::Parts;
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;

    #[derive(Clone, Debug)]
    struct Counter {
        count: i64,
        enabled: bool,
        label: Option<String>,
    }

    impl State for Counter {
        const KEY: &'static str = "test.counter";

        fn defaults() -> Value {
            json!({ "count": 0, "enabled": true })
        }

        fn from_value(value: &Value) -> Self {
            Self {
                count: field(value.get("count"), || 0),
                enabled: field(value.get("enabled"), || true),
                label: field(value.get("label"), || None),
            }
        }

        fn to_value(&self) -> Value {
            let mut object = Map::new();
            insert(&mut object, "count", &self.count);
            insert(&mut object, "enabled", &self.enabled);
            insert(&mut object, "label", &self.label);
            Value::Object(object)
        }
    }
    fn respond(op: &Op) {
        let Ok(value) = serde_json::to_value(op) else {
            return;
        };
        let Some(path) = value["response"].as_str() else {
            return;
        };
        let response = json!({
            "ok": true,
            "value": { "count": 7 },
            "error": Value::Null,
        });
        if let Ok(text) = serde_json::to_string(&response) {
            let _ = fs::write(path, text);
        }
    }

    fn context() -> Context {
        let data_dir = PathBuf::from("/tmp/noctalia-rs-state-unit");
        let _ = fs::create_dir_all(&data_dir);
        let mut ctx = Context::new(
            Parts {
                entry: "entry".to_owned(),
                event: "update".to_owned(),
                payload: Value::Null,
                config: Value::Null,
                state: Value::Null,
                host: Value::Null,
                data_dir,
            },
            Value::Null,
        );
        ctx.set_op_sink(Arc::new(respond));
        ctx
    }

    #[test]
    fn load_merges_stored_fields_over_defaults() {
        let state = load::<Counter>(&context());
        assert_eq!(state.count, 7);
        assert!(state.enabled);
        assert_eq!(state.label, None);
    }

    #[test]
    fn save_writes_the_whole_typed_table() {
        let state = Counter {
            count: 3,
            enabled: false,
            label: None,
        };
        let mut ops = Vec::new();
        save(&state, &mut ops);
        assert_eq!(ops.len(), 1);
        let value = serde_json::to_value(&ops[0]).expect("serialize op");
        assert_eq!(value["op"], json!("state_set"));
        assert_eq!(value["key"], json!("test.counter"));
        assert_eq!(value["value"], json!({ "count": 3, "enabled": false }));
    }

    #[test]
    fn watch_uses_the_state_key() {
        let mut ops = Vec::new();
        watch::<Counter>(&mut ops, "on_change");
        let value = serde_json::to_value(&ops[0]).expect("serialize op");
        assert_eq!(value["path"], json!(["state", "watch"]));
        assert_eq!(value["args"][0], json!("test.counter"));
        assert_eq!(value["args"][1], json!({ "__host_callback": "on_change" }));
    }
}
