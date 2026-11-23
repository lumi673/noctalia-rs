use std::time::{Instant, SystemTime, UNIX_EPOCH};

use noctalia_rs::Context;
use serde_json::{Map, Value};

static EN: std::sync::OnceLock<Value> = std::sync::OnceLock::new();

fn translations() -> &'static Value {
    EN.get_or_init(|| {
        serde_json::from_str(include_str!("../translations/mihomo-en.json"))
            .unwrap_or_default()
    })
}

fn lookup<'a>(root: &'a Value, key: &str) -> Option<&'a Value> {
    let mut cursor = root.get(key);
    if cursor.is_some() {
        return cursor;
    }
    if !key.contains('.') {
        return None;
    }
    let mut parts = key.split('.');
    cursor = root.get(parts.next().unwrap_or_default());
    for part in parts {
        cursor = cursor.and_then(|value| value.get(part));
    }
    cursor
}

pub fn tr(key: &str, params: &[(&str, String)]) -> String {
    let mut text = lookup(translations(), key)
        .and_then(Value::as_str)
        .unwrap_or(key)
        .to_owned();
    for (name, replacement) in params {
        text = text.replace(&format!("{{{name}}}"), replacement);
    }
    text
}

pub fn trp(key: &str, count: i64, params: &[(&str, String)]) -> String {
    let suffix = if count == 1 { "one" } else { "other" };
    let full = format!("{key}.{suffix}");
    let mut text = lookup(translations(), &full)
        .and_then(Value::as_str)
        .or_else(|| lookup(translations(), key).and_then(Value::as_str))
        .unwrap_or(key)
        .to_owned();
    text = text.replace("{count}", &count.to_string());
    for (name, replacement) in params {
        text = text.replace(&format!("{{{name}}}"), replacement);
    }
    text
}

pub fn number(value: Option<&Value>) -> Option<f64> {
    match value {
        Some(Value::Number(number)) => number.as_f64(),
        Some(Value::String(text)) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
}

pub fn string(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str)
}

pub fn truthy(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::Bool(true)))
}

pub fn format_number(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 1e16 {
        format!("{}", value as i64)
    } else {
        value.to_string()
    }
}

pub fn tostring(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "nil".to_owned(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Bool(true)) => "true".to_owned(),
        Some(Value::Bool(false)) => "false".to_owned(),
        Some(Value::Number(number)) => number
            .as_f64()
            .map_or_else(|| number.to_string(), format_number),
        Some(Value::Array(_)) | Some(Value::Object(_)) => "table".to_owned(),
    }
}

pub fn tostring_or_empty(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        other => tostring(other),
    }
}

pub fn lua_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(_), Value::Number(_)) => {
            number(Some(a)) == number(Some(b))
        }
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Null, Value::Null) => true,
        _ => false,
    }
}

pub fn floor(value: f64) -> f64 {
    value.floor()
}

pub fn clamp(value: f64, min: f64, max: f64) -> f64 {
    value.clamp(min, max)
}

pub fn trim(value: &str) -> String {
    value.trim().to_owned()
}

pub fn config(ctx: &Context, key: &str) -> Value {
    ctx.config.get(key).cloned().unwrap_or(Value::Null)
}

pub fn state(ctx: &Context, key: &str) -> Value {
    ctx.state.get(key).cloned().unwrap_or(Value::Null)
}

pub fn url_encode(value: &str) -> String {
    let mut result = String::new();
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(*byte, b'-' | b'_' | b'.' | b'~')
        {
            result.push(char::from(*byte));
        } else {
            result.push_str(&format!("%{byte:02X}"));
        }
    }
    result
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub fn url_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
            {
                out.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn tagged(prefix: &str, value: &str) -> String {
    format!("{prefix}:{}", url_encode(value))
}

pub fn untagged(id: &str, prefix: &str) -> Option<String> {
    id.strip_prefix(&format!("{prefix}:"))
        .map(|value| url_decode(value))
}

pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn clock() -> Instant {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).clone()
}

pub fn clock_seq() -> f64 {
    (clock().elapsed().as_secs_f64() * 1_000_000.0).floor()
}

pub fn unix_time() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub fn format_rate(bytes_per_second: Option<&Value>) -> String {
    let value = number(bytes_per_second).unwrap_or(0.0);
    if value >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.2} GB/s", value / (1024.0 * 1024.0 * 1024.0))
    } else if value >= 1024.0 * 1024.0 {
        format!("{:.1} MB/s", value / (1024.0 * 1024.0))
    } else if value >= 1024.0 {
        format!("{:.1} KB/s", value / 1024.0)
    } else {
        format!("{} B/s", value as i64)
    }
}

pub fn format_total(bytes: Option<&Value>) -> String {
    let value = number(bytes).unwrap_or(0.0);
    if value >= 1024.0_f64.powi(4) {
        format!("{:.2} TB", value / 1024.0_f64.powi(4))
    } else if value >= 1024.0_f64.powi(3) {
        format!("{:.2} GB", value / 1024.0_f64.powi(3))
    } else if value >= 1024.0 * 1024.0 {
        format!("{:.1} MB", value / (1024.0 * 1024.0))
    } else if value >= 1024.0 {
        format!("{:.1} KB", value / 1024.0)
    } else {
        format!("{} B", value as i64)
    }
}

pub fn format_memory(bytes: Option<&Value>) -> String {
    let value = number(bytes).unwrap_or(0.0);
    if value <= 0.0 {
        "—".to_owned()
    } else {
        format_total(bytes)
    }
}

pub fn set_seq(command: &mut Map<String, Value>) {
    command.insert("seq".to_owned(), Value::from(clock_seq() as i64));
}
