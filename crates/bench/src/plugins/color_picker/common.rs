use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

use noctalia_rs::{Context, Op};
use serde_json::Value;

pub fn tr(key: &str, params: &[(&str, String)]) -> String {
    static EN: OnceLock<Value> = OnceLock::new();
    let root = EN.get_or_init(|| {
        serde_json::from_str(include_str!(
            "../translations/color-picker-en.json"
        ))
        .unwrap_or_default()
    });

    let mut value = root.get(key).and_then(Value::as_str);
    if value.is_none() && key.contains('.') {
        let mut parts = key.split('.');
        let mut cursor = root.get(parts.next().unwrap_or_default());
        for part in parts {
            cursor = cursor.and_then(|item| item.get(part));
        }
        value = cursor.and_then(Value::as_str);
    }

    let mut text = value.unwrap_or(key).to_owned();
    for (name, replacement) in params {
        text = text.replace(&format!("{{{name}}}"), replacement);
    }
    text
}

pub fn lua_truthy(value: Option<&Value>) -> bool {
    !matches!(value, None | Some(Value::Null) | Some(Value::Bool(false)))
}

pub fn tonumber(value: Option<&Value>) -> Option<f64> {
    match value {
        Some(Value::Number(number)) => number.as_f64(),
        Some(Value::String(text)) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
}

pub fn lua_number_string(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_owned();
    }
    if value.is_infinite() {
        return if value.is_sign_positive() {
            "inf".to_owned()
        } else {
            "-inf".to_owned()
        };
    }
    if value.fract() == 0.0 && value.abs() < 1e14 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}

pub fn lua_tostring(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "nil".to_owned(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => {
            lua_number_string(number.as_f64().unwrap_or_default())
        }
        Some(Value::Bool(true)) => "true".to_owned(),
        Some(Value::Bool(false)) => "false".to_owned(),
        Some(Value::Array(_)) | Some(Value::Object(_)) => "table".to_owned(),
    }
}

pub fn lua_sub(text: &str, start: usize, end: usize) -> String {
    if start == 0 || start > end {
        return String::new();
    }
    let bytes = text.as_bytes();
    if start > bytes.len() {
        return String::new();
    }
    let stop = end.min(bytes.len());
    String::from_utf8_lossy(&bytes[start - 1..stop]).into_owned()
}

fn state() -> MutexGuard<'static, BTreeMap<String, Value>> {
    static STATE: OnceLock<Mutex<BTreeMap<String, Value>>> = OnceLock::new();
    STATE
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

pub fn state_get(ctx: &Context, key: &str) -> Option<Value> {
    state()
        .get(key)
        .cloned()
        .or_else(|| ctx.state.get(key).cloned())
        .filter(|value| !value.is_null())
}

pub fn state_cache(key: &str, value: Value) {
    state().insert(key.to_owned(), value);
}

pub fn state_set(key: &str, value: Value, ops: &mut Vec<Op>) {
    state().insert(key.to_owned(), value.clone());
    ops.push(Op::state(key, value));
}
