use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::result::Result as StdResult;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use noctalia_rs::{ByteBuf, Context, Op, Parts, Value};
use serde_json::json;

type TestResult = StdResult<(), Box<dyn Error>>;

struct Dir(PathBuf);

impl Dir {
    fn new(label: &str) -> StdResult<Self, Box<dyn Error>> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path =
            env::temp_dir().join(format!("noctalia-host-{label}-{nanos}"));
        fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn context(dir: &Dir, responder: fn(&[String]) -> Value) -> Context {
    let parts = Parts {
        entry: "service".to_owned(),
        event: "render".to_owned(),
        payload: json!({ "source": "load" }),
        config: json!({}),
        state: json!({}),
        host: json!({
            "pluginDir": "/plugins/example",
            "pluginDataDir": Value::Null,
            "setting": { "shell.cached": "snapshot" }
        }),
        data_dir: dir.0.clone(),
    };
    let mut ctx = Context::new(parts, json!({}));
    ctx.set_op_sink(Arc::new(move |op| {
        let Ok(value) = serde_json::to_value(op) else {
            return;
        };
        let Some(path) = value.get("response").and_then(Value::as_str) else {
            return;
        };
        let query_path: Vec<String> = value
            .get("path")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let response = json!({
            "ok": true,
            "value": responder(&query_path),
            "error": Value::Null
        });
        if let Ok(text) = serde_json::to_string(&response) {
            let _ = fs::write(path, text);
        }
    }));
    ctx
}

#[test]
fn synchronous_queries_expose_host_return_values() -> TestResult {
    let dir = Dir::new("values")?;
    let ctx = context(&dir, |path| match path.first().map(String::as_str) {
        Some("fuzzyScore") => json!(12.5),
        Some("runAsync") => json!(false),
        Some("getSetting") => json!("fresh"),
        Some("state") => json!({ "outside": [1, 2] }),
        Some("isValidTimezone") => json!(true),
        Some("loadFont") => json!("Example Font"),
        _ => Value::Null,
    });

    assert_eq!(ctx.fuzzy_score("abc", "axbyc"), Some(12.5));
    assert!(!ctx.run_async_start(json!("run"), None, Some(250.0)));
    assert_eq!(ctx.setting_value("shell.other"), Some(json!("fresh")));
    assert_eq!(ctx.setting_value("shell.cached"), Some(json!("snapshot")));
    assert_eq!(
        ctx.global_state("external.state"),
        Some(json!({ "outside": [1, 2] }))
    );
    assert!(ctx.is_valid_timezone("Europe/Berlin"));
    assert_eq!(
        ctx.load_font_result("fonts/x.ttf").0.as_deref(),
        Some("Example Font")
    );
    assert_eq!(ctx.plugin_data_dir(), None);
    assert_eq!(ctx.plugin_dir_result().as_deref(), None);
    Ok(())
}

#[test]
fn file_results_preserve_value_and_error() -> TestResult {
    let dir = Dir::new("files")?;
    let ctx = context(&dir, |path| match path.first().map(String::as_str) {
        Some("readFile") => {
            json!({ "__noctalia_rs_bytes": "89504e470d0a1a0a" })
        }
        Some("writeFile" | "mkdirAll" | "removeFile" | "renameFile") => {
            json!(false)
        }
        _ => Value::Null,
    });

    let (bytes, error) = ctx.read_file_bytes_result("image.png");
    assert_eq!(error, None);
    let Some(raw) = bytes else {
        return Err("raw file bytes are missing".into());
    };
    assert_eq!(raw.as_bytes(), b"\x89PNG\r\n\x1a\n");
    assert!(!ctx.write_file_result("x", "y").0);
    assert!(!ctx.mkdir_all_result("x").0);
    assert!(!ctx.remove_file_result("x").0);
    assert!(!ctx.rename_file_result("a", "b").0);
    Ok(())
}

#[test]
fn byte_ops_cross_the_json_bridge_as_markers() -> TestResult {
    let bytes = ByteBuf::new(vec![0x00, 0xff]);
    let copy =
        serde_json::to_value(Op::copy_bytes(bytes.clone(), "image/png"))?;
    let copy_args = copy.get("args").and_then(Value::as_array);
    assert_eq!(
        copy_args.and_then(|values| values.first()),
        Some(&json!({ "__noctalia_rs_bytes": "00ff" }))
    );

    let http = serde_json::to_value(Op::http_bytes(
        json!({ "url": "https://example.test" }),
        bytes,
        Some("done"),
    ))?;
    let request = http
        .get("args")
        .and_then(Value::as_array)
        .and_then(|values| values.first());
    assert_eq!(
        request.and_then(|value| value.get("body")),
        Some(&json!({ "__noctalia_rs_bytes": "00ff" }))
    );
    Ok(())
}

#[test]
fn render_source_distinguishes_load_pass() -> TestResult {
    let dir = Dir::new("render")?;
    let ctx = context(&dir, |_| Value::Null);
    assert_eq!(ctx.render_source(), "load");
    assert!(ctx.is_load_event());
    Ok(())
}
