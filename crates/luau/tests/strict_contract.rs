//! Contract checks for the strict Luau emitter.
//!
//! Strict emit has no bridge and no helper, so it must pass real closures
//! where plugin API 57 takes a function, must not define `onHostCallback`, and
//! must resolve host members against the plugin API table.

use std::path::PathBuf;

use noctalia_rs::Context;
use noctalia_rs::compat::Parts;
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
        "__noctalia_rs",
    ] {
        assert!(
            !source.contains(forbidden),
            "strict emit contains `{forbidden}`:\n{source}"
        );
    }
}

#[test]
fn strict_imported_ui_markers_use_action_globals() {
    let handler = r#"
use noctalia_rs::ui::{call, callback};

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "action" {
        return Ok(vec![Op::log("action")]);
    }
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    let id = callback("on_click");
    let tree = ui::row(json!({ "onClick": call("refresh") }), Vec::new());
    let _ = id;
    Ok(vec![Op::BarRender { tree }])
}
"#;
    let source = strict_source(handler, EntryKind::Widget);
    assert_strict_contract(&source);
    assert!(source.contains("__strict_action_refresh"), "{source}");
    assert!(source.contains("__strict_callback"), "{source}");
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
fn strict_handles_emit_parseable_direct_api_calls() {
    let source = strict_source(
        r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "update" {
        let run = ctx.run(json!(["echo"]), Some("run"), Some(250i64));
        let http = ctx.http(json!({ "url": "x" }), "http");
        let stream = ctx.http_stream(json!({ "url": "x" }), "line", "close");
        let watch = ctx.state_watch("watched", "watch");
        run.stop();
        http.stop();
        stream.stop();
        watch.stop();
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
        source.contains("local function __strict_run_handle"),
        "{source}"
    );
    assert!(source.contains("noctalia.httpStream("), "{source}");
    assert!(source.contains("state.watch("), "{source}");
    let update = ops(&source, "update", json!({}));
    assert!(update.iter().any(|op| op["path"] == json!(["runAsync"])));
    assert!(update.iter().any(|op| op["path"] == json!(["http"])));
    assert!(
        update
            .iter()
            .any(|op| op["path"] == json!(["state", "watch"])),
        "{update:?}"
    );
}

#[test]
fn strict_stop_suppresses_only_the_named_handle_callback() {
    let source = strict_source(
        r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "update" {
        let stopped = ctx.run(json!(["echo"]), Some("stopped"), None);
        let live = ctx.run(json!(["echo"]), Some("live"), None);
        stopped.stop();
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
    let update = ops(&source, "update", json!({}));
    let runs = update
        .iter()
        .filter(|op| op["path"] == json!(["runAsync"]))
        .collect::<Vec<_>>();
    assert_eq!(runs.len(), 2);
    let stopped = runs[0]["args"][1]["__host_callback"]
        .as_str()
        .expect("stopped id")
        .to_owned();
    let live = runs[1]["args"][1]["__host_callback"]
        .as_str()
        .expect("live id")
        .to_owned();
    let suppressed = ops(
        &source,
        "host_callback",
        json!({ "id": stopped, "args": [] }),
    );
    assert!(
        suppressed.iter().all(|op| op["op"] != json!("log")),
        "{suppressed:?}"
    );
    let delivered =
        ops(&source, "host_callback", json!({ "id": live, "args": [] }));
    assert!(
        delivered
            .iter()
            .any(|op| op["op"] == json!("log") && op["message"] == "live"),
        "{delivered:?}"
    );
}

#[test]
fn strict_stream_stop_sends_no_internal_host_call() {
    let source = strict_source(
        r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "update" {
        let stream = ctx.http_stream(json!({ "url": "x" }), "line", "close");
        stream.stop();
    }
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    Ok(Vec::new())
}
"#,
        EntryKind::Service,
    );
    assert_strict_contract(&source);
    assert!(
        source.contains("if handle then handle.stop() end"),
        "{source}"
    );
    let update = ops(&source, "update", json!({}));
    assert!(
        update.iter().all(|op| op["root"] != json!("__noctalia_rs")),
        "{update:?}"
    );
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
fn strict_typed_config_reads_the_injected_table() {
    let handler = r#"
#[derive(noctalia_rs::Config)]
struct Settings {
    #[config(key = "message", default = "fallback")]
    message: String,
    #[config(key = "enabled", default = false)]
    enabled: bool,
    #[config(key = "count", default = 2)]
    count: u32,
}

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let settings = Settings::from_ctx(ctx);
    Ok(vec![Op::log(format!("{}:{}:{}", settings.message, settings.enabled, settings.count))])
}
"#;
    let compile_typed = |config: &'static str| {
        let source =
            format!("use noctalia_rs::{{Context, Op, Result}};\n{handler}\n");
        let assets = Vec::new();
        let keys = ["message", "enabled", "count"];
        compile(
            &source,
            &Options {
                handler: "handler",
                entry: "strict",
                state_key: "strict.state",
                kind: EntryKind::Widget,
                strings: &keys,
                settings: &keys,
                module: "main",
                modules: &[],
                native: NativeMode::Deny,
                strict: Some(StrictOptions {
                    config,
                    translations: TRANSLATIONS,
                    assets: &assets,
                }),
            },
        )
        .expect("compile typed config")
        .source
    };

    let populated = compile_typed(
        "local CONFIG = { [\"message\"] = \"from host\", [\"enabled\"] = false, [\"count\"] = 9 }\n",
    );
    assert_strict_contract(&populated);
    assert!(!populated.contains("noctalia.getConfig"), "{populated}");
    let messages = ops(&populated, "render", json!({}));
    let log = messages
        .iter()
        .find(|op| op["op"] == "log")
        .expect("log op");
    let message = log["message"].as_str().expect("message");
    assert!(message.contains("from host"), "{message}");
    assert!(message.contains("false:9"), "{message}");

    let defaults = compile_typed("local CONFIG = { [\"enabled\"] = false }\n");
    let messages = ops(&defaults, "render", json!({}));
    let log = messages
        .iter()
        .find(|op| op["op"] == "log")
        .expect("log op");
    let message = log["message"].as_str().expect("message");
    assert!(message.contains("fallback"), "{message}");
    assert!(message.contains("false:2"), "{message}");

    let wrong_type = compile_typed(
        "local CONFIG = { [\"enabled\"] = \"true\", [\"count\"] = 9 }\n",
    );
    let messages = ops(&wrong_type, "render", json!({}));
    let log = messages
        .iter()
        .find(|op| op["op"] == "log")
        .expect("log op");
    let message = log["message"].as_str().expect("message");
    assert!(message.contains("false:9"), "{message}");
}

#[test]
fn strict_rejects_a_byte_marker_table() {
    let source = r#"
noctalia.writeFile("/tmp/x", { __noctalia_rs_bytes = "ff00" })
"#;
    let error = noctalia_rs_luau::dispatch_strict(
        &context("update", json!({})),
        &[("strict", source)],
    )
    .expect_err("byte marker");
    assert!(error.to_string().contains("__noctalia_rs_bytes"), "{error}");
}

#[test]
fn strict_utf8_text_conversion_validates_at_runtime() {
    let handler = |bytes: &str| {
        format!(
            r#"
use noctalia_rs::{{Bytes, Utf8Text}};

fn handler(_ctx: &Context) -> Result<Vec<Op>> {{
    let text = Utf8Text::from_bytes(Bytes::new(vec![{bytes}]))?;
    Ok(vec![Op::log(text.as_str())])
}}
"#
        )
    };
    let valid = strict_source(&handler("104u8"), EntryKind::Service);
    assert_strict_contract(&valid);
    mlua::Lua::new()
        .load(&valid)
        .into_function()
        .expect("generated script parses");
    let values = ops(&valid, "update", json!({}));
    let messages = values
        .into_iter()
        .filter(|op| op["op"] == json!("log"))
        .map(|op| op["message"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    assert!(
        !messages.is_empty() && messages.iter().all(|item| item == "h"),
        "{messages:?}"
    );

    let invalid = strict_source(&handler("255u8"), EntryKind::Service);
    let error = noctalia_rs_luau::dispatch_strict(
        &context("update", json!({})),
        &[("strict", &invalid)],
    )
    .expect_err("invalid UTF-8");
    assert!(error.to_string().contains("invalid UTF-8"), "{error}");
}

#[test]
fn strict_byte_as_str_falls_back_and_byte_arrays_validate_as_text() {
    let source = strict_source(
        r#"
use noctalia_rs::{Bytes, Utf8Text};

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let raw = Bytes::new(vec![255u8]);
    let fallback = raw.as_str().unwrap_or("fallback");
    let text = Utf8Text::try_from(&[104u8]).unwrap();
    Ok(vec![Op::log(fallback), Op::log(text.as_str())])
}
"#,
        EntryKind::Service,
    );
    assert_strict_contract(&source);
    let values = ops(&source, "update", json!({}));
    let messages = values
        .into_iter()
        .filter(|op| op["op"] == json!("log"))
        .map(|op| op["message"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    assert!(
        messages.iter().any(|item| item == "fallback"),
        "{messages:?}"
    );
    assert!(messages.iter().any(|item| item == "h"), "{messages:?}");
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
        r#"
use noctalia_rs::json::{Array, Object};
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "render" {
        let absent: Option<i64> = None;
        let object = json!({ "absent": absent, "present": Some(1) });
        let mut model = Object::new();
        model.insert("absent", None::<i64>);
        let mut array = Array::new();
        array.push(object);
        return Ok(vec![Op::log(serde_json::to_string(&array)?)]);
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

#[test]
fn strict_json_model_omits_absent_keys_at_runtime() {
    let source = strict_source(
        r#"
use noctalia_rs::json::{Array, Object};
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "render" {
        let maybe: Option<i64> = None;
        let from_macro = json!({ "present": Some(7), "missing": None, "maybe": maybe });
        let mut object = Object::new();
        object.insert("present", Some(8));
        object.insert("missing", None::<i64>);
        let mut array = Array::new();
        array.push(9);
        let text = format!(
            "{}|{}|{}",
            serde_json::to_string(&from_macro)?,
            serde_json::to_string(&object)?,
            serde_json::to_string(&array)?
        );
        return Ok(vec![Op::log(text)]);
    }
    Ok(Vec::new())
}
"#,
        EntryKind::Widget,
    );
    assert_strict_contract(&source);
    let values = ops(&source, "render", json!({}));
    let messages = values
        .iter()
        .filter(|op| op["op"] == json!("log"))
        .map(|op| op["message"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert!(!messages.is_empty(), "{source}");
    assert!(
        messages
            .iter()
            .all(|message| *message == r#"{"present":7}|{"present":8}|[9]"#),
        "{messages:?}\n{source}"
    );
}

#[test]
fn strict_derived_state_script_parses_and_writes_state() {
    let handler = r#"
#[derive(State)]
#[state(key = "strict.counter")]
struct Counter {
    #[state(default = 5)]
    count: i64,
    #[state(default = false)]
    enabled: bool,
    label: Option<String>,
}

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "update" {
        let mut state = Counter::load(ctx);
        state.count += 1;
        let mut ops = Vec::new();
        state.save(&mut ops);
        return Ok(ops);
    }
    Ok(Vec::new())
}
"#;
    let source = strict_source(handler, EntryKind::Service);
    assert_strict_contract(&source);
    assert!(
        source.contains("noctalia.state.get(\"strict.counter\")"),
        "{source}"
    );
    let values = ops(&source, "update", json!({}));
    let written = values
        .iter()
        .find(|value| value["op"] == json!("state_set"))
        .expect("state_set op");
    assert_eq!(written["key"], json!("strict.counter"));
    assert_eq!(written["value"]["count"], json!(6));
    assert_eq!(written["value"]["enabled"], json!(false));
    assert!(written["value"].get("label").is_none());
}

#[test]
fn strict_local_helpers_run_recursion_and_ops_in_expression_position() {
    let handler = r#"
fn fib(n: i64) -> i64 {
    if n <= 1 { n } else { fib(n - 1) + fib(n - 2) }
}

fn build(ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::log(ctx.event.as_str())])
}

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let value = fib(8);
    let ops = build(ctx)?;
    let _ = ops;
    Ok(vec![Op::log(value.to_string())])
}
"#;
    let source = strict_source(handler, EntryKind::Widget);
    assert!(source.contains("__noctalia_helper_main__fib"), "{source}");
    assert!(source.contains("__noctalia_helper_main__build"), "{source}");
    let produced = ops(&source, "render", json!({}));
    let messages = produced
        .iter()
        .filter_map(|op| op.get("message").and_then(serde_json::Value::as_str))
        .collect::<Vec<_>>();
    assert!(messages.starts_with(&["render", "21"]), "{produced:?}");
}

const CROSS_ACCEPT: &str = r#"
fn subscribe(ctx: &Context, ops: &mut Vec<Op>) {
    let streaming = ctx.run_stream_start(json!(["tail", "-f"]), "lines");
    if !streaming {
        ops.push(Op::log("failed"));
    }
}

fn load(ctx: &Context, ops: &mut Vec<Op>) {
    subscribe(ctx, ops);
}

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "render" {
        let mut ops = Vec::new();
        let flag: Option<u8> = None;
        flag.unwrap_or_else(|| {
            load(ctx, &mut ops);
            1
        });
        ops.push(Op::log("subscribed"));
        return Ok(ops);
    }
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    Ok(Vec::new())
}
"#;

const CROSS_OPS_IF: &str = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "render" {
        let vertical = ctx.bar_vertical();
        let mut ops = vec![if vertical {
            Op::BarRender { tree: ui::column(json!({}), Vec::new()) }
        } else {
            Op::BarRender { tree: ui::row(json!({}), Vec::new()) }
        }];
        ops.push(Op::BarTooltip { value: json!("tip") });
        return Ok(ops);
    }
    Ok(Vec::new())
}
"#;

#[test]
fn strict_and_auto_agree_on_an_inlined_acceptance_value() {
    let auto = compile_mode(CROSS_ACCEPT, false);
    let strict = compile_mode(CROSS_ACCEPT, true);
    let auto_ops = normalized(&render_ops(&auto, false));
    let strict_ops = normalized(&render_ops(&strict, true));
    assert_eq!(strict_ops, auto_ops);
}

#[test]
fn strict_and_auto_agree_on_a_vec_of_ops_from_an_if() {
    let auto = compile_mode(CROSS_OPS_IF, false);
    let strict = compile_mode(CROSS_OPS_IF, true);
    let auto_ops = normalized(&render_ops(&auto, false));
    let strict_ops = normalized(&render_ops(&strict, true));
    assert_eq!(strict_ops, auto_ops);
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

fn position_handler(method: &str, position: usize) -> String {
    let call = match method {
        "run_async_start" => "ctx.run_async_start(\"x\", \"cb\", None)",
        "run_stream_start" => "ctx.run_stream_start(\"x\", \"cb\")",
        _ => "ctx.http_start(\"x\", \"cb\")",
    };
    let body = match position {
        1 => format!(
            "if !{call} {{ return Ok(Vec::new()); }}\n        return Ok(Vec::new());"
        ),
        2 => format!("return Ok(vec![Op::log({call}.to_string())]);"),
        3 => format!(
            "let name = if {call} {{ \"on\" }} else {{ \"off\" }};\n        return Ok(vec![Op::log(name)]);"
        ),
        4 => format!(
            "let name = match {call} {{ true => \"on\", false => \"off\" }};\n        return Ok(vec![Op::log(name)]);"
        ),
        _ => format!("{call};\n        return Ok(Vec::new());"),
    };
    format!(
        "fn handler(ctx: &Context) -> Result<Vec<Op>> {{\n    if ctx.event == \"render\" {{\n        {body}\n    }}\n    if ctx.event == \"host_callback\" {{\n        return Ok(vec![Op::log(\"callback\")]);\n    }}\n    Ok(Vec::new())\n}}\n"
    )
}

fn compile_auto(handler: &str) -> Result<String, String> {
    let source = format!(
        "use noctalia_rs::{{Context, Op, Result}};\nuse serde_json::{{Value, json}};\n\n{handler}\n"
    );
    compile(
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
            native: NativeMode::Allow,
            strict: None,
        },
    )
    .map(|compiled| compiled.source)
    .map_err(|error| error.render("strict.rs"))
}

#[test]
fn strict_start_positions_emit_or_error_at_the_span() {
    for method in ["run_async_start", "run_stream_start", "http_start"] {
        let host_call = match method {
            "run_async_start" => "noctalia.runAsync",
            "run_stream_start" => "noctalia.runStream",
            _ => "noctalia.http",
        };
        for position in 1..=5 {
            let handler = position_handler(method, position);
            let strict = compile_mode(&handler, true);
            assert!(
                strict.contains(host_call),
                "{method} position {position} has no {host_call}:\n{strict}"
            );
            let strict_ops = noctalia_rs_luau::dispatch_strict(
                &context("render", json!({})),
                &[("strict", &strict)],
            )
            .unwrap_or_else(|error| {
                panic!("{method} position {position} strict dispatch: {error}")
            });
            let strict_ops = normalized(
                &strict_ops
                    .iter()
                    .map(|op| serde_json::to_value(op).expect("serialize"))
                    .collect::<Vec<_>>(),
            );
            let auto = match compile_auto(&handler) {
                Ok(source) => source,
                Err(error) => {
                    assert!(
                        error.contains("must be discarded"),
                        "{method} position {position} auto failed for another reason: {error}"
                    );
                    assert!(
                        error.contains("strict.rs:"),
                        "{method} position {position} auto has no position: {error}"
                    );
                    continue;
                }
            };
            let auto_ops = noctalia_rs_luau::dispatch(
                &context("render", json!({})),
                &[("strict", &auto)],
            )
            .unwrap_or_else(|error| {
                panic!("{method} position {position} auto dispatch: {error}")
            });
            let auto_ops = normalized(
                &auto_ops
                    .iter()
                    .map(|op| serde_json::to_value(op).expect("serialize"))
                    .collect::<Vec<_>>(),
            );
            assert_eq!(
                strict_ops, auto_ops,
                "{method} position {position} ops differ\nstrict:\n{strict}\nauto:\n{auto}"
            );
        }
    }
}

#[test]
fn strict_run_without_a_callback_sends_nil() {
    let source = strict_source(
        r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "update" {
        let handle = ctx.run(json!(["echo"]), None, None);
        handle.stop();
    }
    Ok(Vec::new())
}
"#,
        EntryKind::Service,
    );
    assert!(!source.contains("function __strict_callback"), "{source}");
    let update = ops(&source, "update", json!({}));
    let run = update
        .iter()
        .find(|op| op["path"] == json!(["runAsync"]))
        .expect("run op");
    assert_eq!(run["args"][1], serde_json::Value::Null);
}

#[test]
fn strict_handle_keyword_binding_parses() {
    let source = strict_source(
        r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "update" {
        let end = ctx.run(json!(["echo"]), Some("done"), None);
        end.stop();
    }
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    Ok(Vec::new())
}
"#,
        EntryKind::Service,
    );
    assert_strict_contract(&source);
    assert!(source.contains("__kw_end.stop()"), "{source}");
    let update = ops(&source, "update", json!({}));
    assert!(update.iter().any(|op| op["path"] == json!(["runAsync"])));
}

#[test]
fn strict_watch_handle_started_checks_registration_arguments() {
    let source = strict_source(
        r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "update" {
        let empty_key = ctx.state_watch("", "cb");
        let empty_callback = ctx.state_watch("key", "");
        let started = empty_key.started() || empty_callback.started();
        return Ok(vec![Op::log(started.to_string())]);
    }
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    Ok(Vec::new())
}
"#,
        EntryKind::Service,
    );
    assert_strict_contract(&source);
    assert!(
        source.contains("__key ~= \"\" and __callback ~= \"\""),
        "{source}"
    );
    let update = ops(&source, "update", json!({}));
    let log = update
        .iter()
        .find(|op| op["op"] == json!("log"))
        .expect("log op");
    assert_eq!(log["message"], "false");
}
