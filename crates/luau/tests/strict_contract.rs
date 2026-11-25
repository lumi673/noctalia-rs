//! Contract checks for the strict Luau emitter.
//!
//! Strict emit has no bridge and no helper, so it must pass real closures
//! where plugin API 57 takes a function, must not define `onHostCallback`, and
//! must resolve host members against the plugin API table.

use std::path::PathBuf;

use noctalia_rs::{Context, Parts};
use noctalia_rs_compiler::strict::{MemberKind, groups};
use noctalia_rs_compiler::{
    EntryKind, NativeMode, Options, StrictOptions, compile,
};
use serde_json::{Value, json};

const CONFIG: &str = "local CONFIG = {}\n";
const TRANSLATIONS: &str = "local LANGUAGES = {}\nlocal TRANSLATIONS = {}\n";

fn strict_source(handler: &str, kind: EntryKind) -> String {
    let source = format!(
        "use noctalia_rs::{{Context, Op, Result}};\nuse serde_json::{{Value, json}};\n\n{handler}\n"
    );
    let assets: Vec<(&str, u32)> = Vec::new();
    compile(
        &source,
        &Options {
            handler: "handler",
            entry: "strict",
            state_key: "strict.state",
            kind,
            strings: &[],
            settings: &[],
            module: "main",
            modules: &[],
            native: NativeMode::Deny,
            strict: Some(StrictOptions {
                config: CONFIG,
                translations: TRANSLATIONS,
                assets: &assets,
            }),
        },
    )
    .expect("compile")
    .source
}

fn assert_strict_contract(source: &str) {
    for forbidden in [
        "__host_callback",
        "__call =",
        "onHostCallback",
        "bridge.luau",
        "lib/bridge",
    ] {
        assert!(
            !source.contains(forbidden),
            "strict emit contains `{forbidden}`:\n{source}"
        );
    }
}

fn context(event: &str, payload: Value) -> Context {
    Context::new(
        Parts {
            entry: "strict".to_owned(),
            event: event.to_owned(),
            payload,
            config: json!({}),
            state: json!({}),
            host: json!({}),
            data_dir: PathBuf::from("/tmp/noctalia-strict-contract"),
        },
        json!({}),
    )
}

fn ops(source: &str, event: &str, payload: Value) -> Vec<Value> {
    noctalia_rs_luau::dispatch_strict(
        &context(event, payload),
        &[("strict", source)],
    )
    .expect("strict dispatch")
    .iter()
    .map(|op| serde_json::to_value(op).expect("serialize"))
    .collect()
}

#[test]
fn strict_callback_emit_uses_real_closures() {
    let source = strict_source(
        r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "update" {
        ctx.http_start(json!({ "url": "https://example.invalid/" }), "row-1");
    }
    if ctx.event == "host_callback" {
        let id = ctx.payload_str("id").unwrap_or_default();
        return Ok(vec![Op::log(id)]);
    }
    Ok(Vec::new())
}
"#,
        EntryKind::Service,
    );
    assert_strict_contract(&source);
    assert!(
        source.contains("function(...) __strict_callback("),
        "{source}"
    );
    assert!(
        source.contains("function __strict_callback(id, ...)"),
        "{source}"
    );
}

#[test]
fn strict_op_emit_uses_no_marker_tables() {
    let source = strict_source(
        r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "load" {
        return Ok(vec![Op::run_stream(json!(["tail", "-f"]), "stream")]);
    }
    if ctx.event == "host_callback" {
        let id = ctx.payload_str("id").unwrap_or_default();
        return Ok(vec![Op::log(id)]);
    }
    Ok(Vec::new())
}
"#,
        EntryKind::Service,
    );
    assert_strict_contract(&source);
    assert!(source.contains("noctalia.runStream("), "{source}");
}

#[test]
fn strict_action_uses_a_global_name() {
    let source = strict_source(
        r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "render" {
        let tree = ui::button(json!({ "onClick": ui::call("go") }));
        return Ok(vec![Op::BarRender { tree }]);
    }
    if ctx.event == "action" {
        let name = ctx.payload_str("name").unwrap_or_default();
        return Ok(vec![Op::log(name)]);
    }
    Ok(Vec::new())
}
"#,
        EntryKind::Widget,
    );
    assert_strict_contract(&source);
    assert!(
        source.contains("onClick = \"__strict_action_go_"),
        "{source}"
    );
    assert!(source.contains("function __strict_action_go_"), "{source}");
}

#[test]
fn strict_callback_reaches_the_host_callback_branch() {
    let source = strict_source(
        r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "update" {
        ctx.http_start(json!({ "url": "https://example.invalid/" }), "row-1");
    }
    if ctx.event == "host_callback" {
        let id = ctx.payload_str("id").unwrap_or_default();
        let value = ctx.payload_arg(0).and_then(Value::as_str).unwrap_or_default();
        return Ok(vec![Op::log(format!("{id} {value}"))]);
    }
    Ok(Vec::new())
}
"#,
        EntryKind::Service,
    );
    let update = ops(&source, "update", json!({}));
    let http = update
        .iter()
        .find(|op| op["path"] == json!(["http"]))
        .expect("http op");
    let id = http["args"][1]["__host_callback"]
        .as_str()
        .expect("callback id")
        .to_owned();
    let callbacks = ops(
        &source,
        "host_callback",
        json!({ "id": id, "args": ["later"] }),
    );
    let log = callbacks
        .iter()
        .find(|op| op["op"] == json!("log"))
        .expect("log op");
    assert_eq!(log["message"], "row-1 later");
}

#[test]
fn strict_api_table_matches_the_plugin_api() {
    let mut expected = std::collections::BTreeMap::new();
    for member in noctalia_rs::api::members() {
        expected.insert(
            (member.root.to_owned(), member.name.to_owned()),
            kind_name(member.kind),
        );
    }
    let mut actual = std::collections::BTreeMap::new();
    for (root, members) in groups() {
        for (name, kind) in members {
            actual.insert(
                (root.to_owned(), (*name).to_owned()),
                strict_kind(*kind),
            );
        }
    }
    assert_eq!(actual, expected);
}

fn kind_name(kind: noctalia_rs::api::MemberKind) -> &'static str {
    match kind {
        noctalia_rs::api::MemberKind::Void => "void",
        noctalia_rs::api::MemberKind::Bool => "bool",
        noctalia_rs::api::MemberKind::Value => "value",
        noctalia_rs::api::MemberKind::Pair => "pair",
        noctalia_rs::api::MemberKind::Handler => "handler",
        noctalia_rs::api::MemberKind::Namespace => "namespace",
        noctalia_rs::api::MemberKind::Node => "node",
    }
}

fn strict_kind(kind: MemberKind) -> &'static str {
    match kind {
        MemberKind::Void => "void",
        MemberKind::Bool => "bool",
        MemberKind::Value => "value",
        MemberKind::Pair => "pair",
        MemberKind::Handler => "handler",
        MemberKind::Namespace => "namespace",
        MemberKind::Node => "node",
    }
}

const CROSS: &str = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "render" {
        let table = "shadow".to_owned();
        let id = "first".to_owned();
        let first = ui::button(json!({ "onClick": ui::callback(&id) }));
        let id = "second".to_owned();
        let second = ui::button(json!({ "onClick": ui::callback(&id) }));
        let focused = ctx.query_value("noctalia", &["focusedOutputName"], Vec::new());
        let _ = (table, focused);
        let tree = ui::row(
            json!({ "onHover": ui::call("hovered") }),
            vec![first, second],
        );
        return Ok(vec![Op::BarRender { tree }]);
    }
    if ctx.event == "action" {
        let name = ctx.payload_str("name").unwrap_or_default();
        return Ok(vec![Op::log(name)]);
    }
    if ctx.event == "host_callback" {
        let id = ctx.payload_str("id").unwrap_or_default();
        return Ok(vec![Op::log(id)]);
    }
    Ok(Vec::new())
}
"#;

fn compile_mode(handler: &str, strict: bool) -> String {
    let source = format!(
        "use noctalia_rs::{{Context, Op, Result}};\nuse serde_json::{{Value, json}};\n\n{handler}\n"
    );
    let assets: Vec<(&str, u32)> = Vec::new();
    let compiled = compile(
        &source,
        &Options {
            handler: "handler",
            entry: "strict",
            state_key: "strict.state",
            kind: EntryKind::Widget,
            strings: &[],
            settings: &[],
            module: "main",
            modules: &[],
            native: if strict {
                NativeMode::Deny
            } else {
                NativeMode::Allow
            },
            strict: strict.then_some(StrictOptions {
                config: CONFIG,
                translations: TRANSLATIONS,
                assets: &assets,
            }),
        },
    )
    .expect("compile");
    assert!(
        compiled.native.is_empty(),
        "the cross-mode fixture must compile without the helper"
    );
    compiled.source
}

#[test]
fn strict_and_auto_dispatch_the_same_ops() {
    let auto = compile_mode(CROSS, false);
    let strict = compile_mode(CROSS, true);
    println!("{strict}");
    let auto_ops = normalized(&render_ops(&auto, false));
    let strict_ops = normalized(&render_ops(&strict, true));
    assert_eq!(strict_ops, auto_ops);
}

#[test]
fn strict_and_auto_capture_a_reassigned_callback_id() {
    for (source, strict) in [
        (compile_mode(CROSS, false), false),
        (compile_mode(CROSS, true), true),
    ] {
        let ops = render_ops(&source, strict);
        let watch = ops
            .first()
            .and_then(|op| op["args"][1]["__host_callback"].as_str())
            .unwrap_or_default()
            .to_owned();
        let ids = callback_ids(&ops)
            .into_iter()
            .filter(|id| id != &watch)
            .take(2)
            .collect::<Vec<_>>();
        assert_eq!(ids.len(), 2, "{ops:?}");
        let mut logged = Vec::new();
        for id in ids {
            let values = if strict {
                noctalia_rs_luau::dispatch_strict(
                    &context("host_callback", json!({ "id": id, "args": [] })),
                    &[("strict", &source)],
                )
                .expect("strict host callback")
            } else {
                noctalia_rs_luau::dispatch(
                    &context("host_callback", json!({ "id": id, "args": [] })),
                    &[("strict", &source)],
                )
                .expect("auto host callback")
            };
            let value = values
                .iter()
                .map(|op| serde_json::to_value(op).expect("serialize"))
                .find(|op| op["op"] == json!("log"))
                .expect("log op");
            logged
                .push(value["message"].as_str().unwrap_or_default().to_owned());
        }
        assert_eq!(logged, ["first", "second"]);
    }
}

#[test]
fn strict_runtime_reuses_an_acceptance_result_and_handles_bytes_and_queries() {
    let handler = r#"
fn start(ctx: &Context) -> bool {
    ctx.run_stream_start(json!(["tail", "-f"]), "stream")
}

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "update" {
        let streaming = start(ctx);
        let response = format!("{}/q.json", ctx.data_dir.to_string_lossy());
        let mut ops = Vec::new();
        ops.push(Op::query("q", "noctalia", &["flatpakAppInstalled"], vec![json!("app")], response.clone()));
        if let Ok(raw) = ctx.read_file(&response) {
            ops.push(Op::log(raw));
        }
        let _ = ctx.write_file(&response, "written");
        if let Ok(raw) = ctx.read_file(&response) {
            ops.push(Op::log(raw));
        }
        let _ = ctx.copy_bytes(ByteBuf::new(vec![0u8, 255u8]), "application/octet-stream");
        ops.push(Op::log(streaming.to_string()));
        return Ok(ops);
    }
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    Ok(Vec::new())
}
"#;
    let source = strict_source(handler, EntryKind::Service);
    let values = ops(&source, "update", json!({}));
    let messages = values
        .iter()
        .filter(|op| op["op"] == json!("log"))
        .map(|op| op["message"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert!(
        messages
            .iter()
            .any(|message| message.starts_with("{\"ok\":")),
        "{messages:?}"
    );
    assert!(messages.contains(&"written"), "{messages:?}");
    assert!(messages.contains(&"true"), "{messages:?}");
}

#[test]
fn strict_emit_matrix_parses() {
    let fixtures = [
        r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "update" {
        let config = ctx.config.clone();
        let translations = ctx.translations().clone();
        let _ = (config, translations);
        let value = ctx.query_bool("noctalia", &["outputs"], Vec::new());
        return Ok(vec![Op::log(value.to_string())]);
    }
    Ok(Vec::new())
}
"#,
        r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "render" {
        let name = ctx.payload_str("action").unwrap_or_default();
        let id = format!("row-{name}");
        let tree = ui::row(json!({
            "onHover": ui::call(&name),
            "onClick": ui::callback(&id),
        }), Vec::new());
        return Ok(vec![Op::BarRender { tree }]);
    }
    if ctx.event == "action" {
        return Ok(vec![Op::log("action")]);
    }
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    Ok(Vec::new())
}
"#,
        r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "update" {
        let timeout = Some(100i64);
        if ctx.run_async_start(json!(["echo", "hi"]), "done", timeout) {
            return Ok(vec![Op::log("accepted")]);
        }
        return Ok(Vec::new());
    }
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    Ok(Vec::new())
}
"#,
    ];
    for handler in fixtures {
        let source = strict_source(handler, EntryKind::Widget);
        let _ = ops(&source, "update", json!({}));
    }
}

fn render_ops(source: &str, strict: bool) -> Vec<Value> {
    let ctx = context("render", json!({}));
    let ops = if strict {
        noctalia_rs_luau::dispatch_strict(&ctx, &[("strict", source)])
    } else {
        noctalia_rs_luau::dispatch(&ctx, &[("strict", source)])
    }
    .expect("dispatch");
    ops.iter()
        .map(|op| serde_json::to_value(op).expect("serialize"))
        .collect()
}

fn normalized(ops: &[Value]) -> Vec<Value> {
    ops.iter().map(normalize).collect()
}

fn callback_ids(ops: &[Value]) -> Vec<String> {
    let mut result = Vec::new();
    for op in ops {
        collect_callbacks(op, &mut result);
    }
    result
}

fn collect_callbacks(value: &Value, result: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(id)) = map.get("__host_callback")
                && !result.contains(id)
            {
                result.push(id.clone());
            }
            for (_, item) in map {
                collect_callbacks(item, result);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_callbacks(item, result);
            }
        }
        _ => {}
    }
}

/// Removes the values that legitimately differ between modes.
fn normalize(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            if map.len() == 1
                && let Some(Value::String(_)) = map.get("__call")
            {
                return Value::String("<action>".to_owned());
            }
            let mut out = serde_json::Map::new();
            for (key, item) in map {
                let item = match (key.as_str(), item) {
                    ("__host_callback", Value::String(_)) => {
                        Value::String("<callback>".to_owned())
                    }
                    ("__call", Value::String(_)) => {
                        Value::String("<action>".to_owned())
                    }
                    ("id" | "response", _)
                        if map.get("op") == Some(&json!("query")) =>
                    {
                        Value::String("<query>".to_owned())
                    }
                    _ => normalize(item),
                };
                out.insert(key.clone(), item);
            }
            Value::Object(out)
        }
        Value::Array(items) => {
            Value::Array(items.iter().map(normalize).collect())
        }
        Value::String(text) if text.starts_with("__strict_action_") => {
            Value::String("<action>".to_owned())
        }
        other => other.clone(),
    }
}
