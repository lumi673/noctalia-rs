mod curve;
mod util;

use super::{Case, Result, wrapper};
use noctalia_rs::Op;
use serde_json::{Value, json};

const SOURCES: &[(&str, &str)] = &[("curve.luau", include_str!("curve.luau"))];

const BODY: &str = r#"
local curve = require("./curve.luau")
function update()
  local compiled = curve.compile(fixture.nodes)
  local out = {}
  for index, x in ipairs(fixture.xs) do
    out[index] = string.format("%.6f", curve.eval(compiled, x))
  end
  noctalia.log(json.encode(out))
end
"#;

fn fixture() -> Value {
    let xs = (0..256)
        .map(|index| (index as f64) * 14200.0 / 255.0)
        .collect::<Vec<_>>();
    json!({
        "nodes": [
            { "x": 0, "y": 0 }, { "x": 10, "y": 12.5 }, { "x": 33, "y": 39.3 },
            { "x": 100, "y": 50.6 }, { "x": 350, "y": 63.4 }, { "x": 800, "y": 70 },
            { "x": 1680, "y": 88.3 }, { "x": 3200, "y": 92.1 }, { "x": 4800, "y": 96.8 },
            { "x": 8000, "y": 98.4 }, { "x": 14200, "y": 100 }
        ],
        "xs": xs
    })
}

fn nodes(fixture: &Value) -> Result<Vec<curve::Node>> {
    let values = fixture
        .get("nodes")
        .and_then(Value::as_array)
        .ok_or("curve fixture has no nodes")?;
    values
        .iter()
        .map(|node| {
            Ok(curve::Node {
                x: node
                    .get("x")
                    .and_then(Value::as_f64)
                    .ok_or("curve node has no x")?,
                y: node
                    .get("y")
                    .and_then(Value::as_f64)
                    .ok_or("curve node has no y")?,
            })
        })
        .collect()
}

pub fn case() -> Result<Case> {
    let fixture = fixture();
    Ok(Case {
        workload: "plugin_als_brightness",
        luau_surface: "plugin_als_brightness_luau",
        rust_surface: "plugin_als_brightness_rust",
        modules: SOURCES,
        wrapper: wrapper(&fixture, BODY)?,
        fixture,
        rust: Box::new(|fixture: &Value| {
            let compiled = curve::compile(&nodes(fixture)?);
            let xs = fixture
                .get("xs")
                .and_then(Value::as_array)
                .ok_or("curve fixture has no xs")?;
            let mut out = Vec::with_capacity(xs.len());
            for value in xs {
                let x = value.as_f64().ok_or("curve sample is not numeric")?;
                let y = curve::eval(compiled.as_ref(), x)
                    .ok_or("curve eval failed")?;
                out.push(format!("{y:.6}"));
            }
            Ok(vec![Op::log(serde_json::to_string(&out)?)])
        }),
    })
}
