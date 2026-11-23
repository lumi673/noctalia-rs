mod space_index;

use super::{Case, Result, wrapper};
use noctalia_rs::Op;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

const SOURCES: &[(&str, &str)] =
    &[("space_index.luau", include_str!("space_index.luau"))];

const BODY: &str = r#"
local idx = require("./space_index.luau")
local function index(value)
  if value then return tostring(value) else return "nil" end
end
function update()
  local out = {}
  for modeIndex, mode in ipairs({ "folders", "types" }) do
    for sliceIndex, slice in ipairs(idx.slices(fixture, mode, 8)) do
      out[#out + 1] = string.format("%s|%s|%s|%.0f|%s|%s", mode,
        slice.label, slice.kind, slice.bytes, index(slice.index), slice.color or "")
    end
  end
  local entries = idx.entries(fixture, "folders")
  out[#out + 1] = idx.commonDir(entries, 3)
  noctalia.log(json.encode(out))
end
"#;

fn file(name: &str, bytes: i64, category: i64) -> Value {
    json!({ "n": name, "s": bytes, "k": category })
}

fn fixture() -> Value {
    let child = |name: &str, bytes: i64, files: i64| {
        json!({
            "n": name, "s": bytes, "f": bytes / 5, "u": 0, "nf": files,
            "c": [0, bytes / 2, bytes / 4, 0, 0, bytes / 8, 0, bytes / 8],
            "d": [],
            "t": [file(&format!("{name}/top.mkv"), bytes / 3, 1)]
        })
    };
    json!({
        "n": "root", "s": 1000000, "f": 120000, "u": 1, "nf": 240,
        "c": [300000, 200000, 150000, 50000, 40000, 120000, 30000, 110000],
        "d": [
            child("Videos", 400000, 40),
            child("Pictures", 250000, 60),
            child("Music", 180000, 80),
            child("Code", 50000, 60)
        ],
        "t": [
            file("movie.mkv", 90000, 1),
            file("photo.png", 40000, 2),
            file("song.flac", 30000, 3),
            file("notes.md", 12000, 5),
            file("main.rs", 8000, 6)
        ]
    })
}

fn node(value: &Value) -> Result<space_index::NodeRef> {
    let categories = value
        .get("c")
        .and_then(Value::as_array)
        .ok_or("space node has no category bytes")?;
    if categories.len() != 8 {
        return Err("space node has the wrong category count".into());
    }
    let mut c = [0.0; 8];
    for (slot, value) in c.iter_mut().zip(categories) {
        *slot = value.as_f64().ok_or("space category is not numeric")?;
    }
    let children = value
        .get("d")
        .and_then(Value::as_array)
        .ok_or("space node has no children")?
        .iter()
        .map(node)
        .collect::<Result<Vec<_>>>()?;
    let top = value
        .get("t")
        .and_then(Value::as_array)
        .ok_or("space node has no top files")?
        .iter()
        .map(|item| {
            Ok(space_index::FileRec {
                n: item
                    .get("n")
                    .and_then(Value::as_str)
                    .ok_or("space file has no name")?
                    .to_owned(),
                s: item
                    .get("s")
                    .and_then(Value::as_f64)
                    .ok_or("space file has no size")?,
                k: item
                    .get("k")
                    .and_then(Value::as_i64)
                    .ok_or("space file has no category")?
                    as usize,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Arc::new(Mutex::new(space_index::Node {
        id: String::new(),
        n: value
            .get("n")
            .and_then(Value::as_str)
            .ok_or("space node has no name")?
            .to_owned(),
        s: value
            .get("s")
            .and_then(Value::as_f64)
            .ok_or("space node has no size")?,
        f: value
            .get("f")
            .and_then(Value::as_f64)
            .ok_or("space node has no loose bytes")?,
        u: value
            .get("u")
            .and_then(Value::as_f64)
            .ok_or("space node has no unreadable count")?,
        nf: value
            .get("nf")
            .and_then(Value::as_f64)
            .ok_or("space node has no file count")?,
        c,
        d: children,
        t: top,
        leaf: false,
    })))
}

pub fn case() -> Result<Case> {
    let fixture = fixture();
    Ok(Case {
        workload: "plugin_file_search",
        luau_surface: "plugin_file_search_luau",
        rust_surface: "plugin_file_search_rust",
        modules: SOURCES,
        wrapper: wrapper(&fixture, BODY)?,
        fixture,
        rust: Box::new(|fixture: &Value| {
            let root = node(fixture)?;
            let mut out = Vec::new();
            for mode in ["folders", "types"] {
                for slice in space_index::slices(&root, mode, 8) {
                    let index = slice
                        .index
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "nil".to_owned());
                    out.push(format!(
                        "{}|{}|{}|{:.0}|{}|{}",
                        mode,
                        slice.label,
                        slice.kind,
                        slice.bytes,
                        index,
                        slice.color.unwrap_or_default()
                    ));
                }
            }
            let entries = space_index::entries(&root, "folders", None);
            out.push(space_index::common_dir(&entries, 3));
            Ok(vec![Op::log(serde_json::to_string(&out)?)])
        }),
    })
}
