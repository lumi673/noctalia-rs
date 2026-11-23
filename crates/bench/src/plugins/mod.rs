//! Vendor-free plugin algorithm fixtures used by the paired benchmark.

#![allow(dead_code, unused_assignments, unused_imports, unused_parens)]
#![allow(clippy::all)]

mod als_brightness;
mod case_convert;
mod color_picker;
mod file_search;
mod mihomo_control;
mod vietnamese_lunar_calendar;

use noctalia_rs::Op;
use serde_json::{Value, json};

use super::{Options, Pair, Result, Summary, measure};
use std::fs;
use std::path::Path;

type RustRun = Box<dyn Fn(&Value) -> Result<Vec<Op>>>;

pub struct Case {
    workload: &'static str,
    luau_surface: &'static str,
    rust_surface: &'static str,
    modules: &'static [(&'static str, &'static str)],
    wrapper: String,
    fixture: Value,
    rust: RustRun,
}

pub fn run(
    options: &Options,
    scratch: &Path,
    results: &mut Vec<Summary>,
    pairs: &mut Vec<Pair>,
) -> Result<()> {
    let mut cases = Vec::new();
    cases.push(case_convert::case()?);
    cases.push(color_picker::case()?);
    cases.push(vietnamese_lunar_calendar::case()?);
    cases.push(als_brightness::case()?);
    cases.push(mihomo_control::case()?);
    cases.push(file_search::case()?);

    for case in cases {
        let dir = scratch.join(format!("plugin-{}", case.workload));
        fs::create_dir_all(&dir)?;
        let ctx = super::context(("entry", "update"), &dir, json!({}));
        let mut sources = case.modules.to_vec();
        sources.push(("entry", case.wrapper.as_str()));

        let expected = (case.rust)(&case.fixture)?;
        let actual = noctalia_rs_luau::dispatch(&ctx, &sources)?;
        let expected = canonical(&expected)?;
        let actual = canonical(&actual)?;
        if expected != actual {
            return Err(format!(
                "{} output mismatch before timing\nluau: {}\nrust: {}",
                case.workload, actual, expected
            )
            .into());
        }

        let luau_phase = options.phase(case.luau_surface, false);
        let luau = measure(luau_phase, || {
            let ops = noctalia_rs_luau::dispatch(&ctx, &sources)?;
            std::hint::black_box(ops);
            Ok(())
        })?;
        let rust_phase = options.phase(case.rust_surface, false);
        let rust = measure(rust_phase, || {
            let ops = (case.rust)(&case.fixture)?;
            std::hint::black_box(ops);
            Ok(())
        })?;
        results.push(luau);
        results.push(rust);
        pairs.push(Pair::new(case.workload, rust, luau));
    }
    Ok(())
}

pub fn wrapper(fixture: &Value, body: &str) -> Result<String> {
    let text = serde_json::to_string(fixture)?;
    if text.contains("]==]") {
        return Err("fixture contains a Lua long-string terminator".into());
    }
    Ok(format!(
        "local fixture = json.decode([==[{text}]==])\n{body}\n"
    ))
}

fn canonical(ops: &[Op]) -> Result<String> {
    let value = serde_json::to_value(ops)?;
    Ok(canonical_value(&value))
}

fn canonical_value(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(value) => format!("b{value}"),
        Value::Number(value) => {
            format!("n{:.12}", value.as_f64().unwrap_or(0.0))
        }
        Value::String(value) => format!("s{value:?}"),
        Value::Array(values) => {
            let parts = values.iter().map(canonical_value).collect::<Vec<_>>();
            format!("[{}]", parts.join(","))
        }
        Value::Object(values) => {
            if values.get("op").and_then(Value::as_str) == Some("log")
                && let Some(message) =
                    values.get("message").and_then(Value::as_str)
                && let Ok(parsed) = serde_json::from_str::<Value>(message)
            {
                return format!("jsonlog:{}", canonical_value(&parsed));
            }
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort();
            let parts = keys
                .into_iter()
                .map(|key| format!("{}:{}", key, canonical_value(&values[key])))
                .collect::<Vec<_>>();
            format!("{{{}}}", parts.join(","))
        }
    }
}
