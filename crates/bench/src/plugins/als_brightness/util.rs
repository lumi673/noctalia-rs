use serde_json::Value;

pub fn finite(value: f64) -> bool {
    value.is_finite()
}

pub fn truthy(value: Option<&Value>) -> bool {
    !matches!(value, None | Some(Value::Null | Value::Bool(false)))
}

pub fn as_f64(value: Option<&Value>) -> Option<f64> {
    match value {
        Some(Value::Number(number)) => number.as_f64(),
        _ => None,
    }
}

pub fn lua_number(value: f64) -> String {
    if value == 0.0 {
        "0".to_owned()
    } else {
        value.to_string()
    }
}

pub fn lua_value(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "nil".to_owned(),
        Some(Value::Bool(true)) => "true".to_owned(),
        Some(Value::Bool(false)) => "false".to_owned(),
        Some(Value::Number(number)) => number.to_string(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(_)) | Some(Value::Object(_)) => "table".to_owned(),
    }
}

pub fn trim_end(value: &str) -> &str {
    value.trim_end_matches(char::is_whitespace)
}

pub fn trim_start(value: &str) -> &str {
    value.trim_start_matches(char::is_whitespace)
}

pub fn tonumber(value: &str) -> Option<f64> {
    let text = value.trim();
    if let Some(hex) = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .or_else(|| text.strip_prefix("-0x"))
        .or_else(|| text.strip_prefix("-0X"))
    {
        let negative = text.starts_with('-');
        return i64::from_str_radix(hex, 16).ok().map(|number| {
            if negative {
                -number as f64
            } else {
                number as f64
            }
        });
    }
    text.parse::<f64>().ok()
}

pub fn round_half_away(value: f64) -> f64 {
    if value >= 0.0 {
        (value + 0.5).floor()
    } else {
        -((-value + 0.5).floor())
    }
}

pub fn is_iio_device(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("iio:device") else {
        return false;
    };
    !rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_digit())
}
