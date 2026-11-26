use std::path::PathBuf;

use noctalia_rs::{Context, Op, Parts};
use noctalia_rs_compiler::{EntryKind, Options, compile};
use serde_json::{Value, json};

#[allow(clippy::too_many_arguments)]
fn context(entry: &str, event: &str, payload: Value, state: Value) -> Context {
    Context::new(
        Parts {
            entry: entry.to_owned(),
            event: event.to_owned(),
            payload,
            config: json!({ "message": "Hello" }),
            state,
            host: json!({}),
            data_dir: PathBuf::from("/tmp/noctalia-compiled"),
        },
        json!({ "greet": { "label": "Hello {name}" } }),
    )
}

fn compiled_source(entry: &str, handler: &str, kind: EntryKind) -> String {
    let source = format!(
        "use noctalia_rs::{{Context, Op, Result}};\nuse serde_json::json;\n\n{handler}\n"
    );
    compile(
        &source,
        &Options {
            handler: "handler",
            entry,
            state_key: "hello.state",
            kind,
            strings: &["message"],
            settings: &["message"],
            module: "main",
            modules: &[],
            native: noctalia_rs_compiler::NativeMode::Deny,
            strict: None,
        },
    )
    .expect("compile")
    .source
}

fn ops(entry: &str, source: &str, ctx: &Context) -> Vec<Value> {
    noctalia_rs_luau::dispatch(ctx, &[(entry, source)])
        .expect("dispatch")
        .iter()
        .filter(|op| !matches!(op, Op::HostCall { .. }))
        .map(|op| serde_json::to_value(op).expect("serialize"))
        .collect()
}

fn rust_ops(ops: Vec<Op>) -> Vec<Value> {
    ops.iter()
        .filter(|op| !matches!(op, Op::HostCall { .. }))
        .map(|op| serde_json::to_value(op).expect("serialize"))
        .collect()
}

#[test]
fn hello_widget_matches_the_native_handler() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "render" || ctx.event == "click" {
        let message = ctx.config_str("message").unwrap_or_default().into_owned();
        Ok(vec![Op::BarText { text: message }])
    } else {
        Ok(Vec::new())
    }
}
"#;
    let source = compiled_source("hello_widget", handler, EntryKind::Widget);
    let state = json!({});

    let load = context(
        "hello_widget",
        "render",
        json!({ "source": "load" }),
        state.clone(),
    );
    let update = context(
        "hello_widget",
        "render",
        json!({ "source": "update" }),
        state.clone(),
    );
    let expected = {
        let mut ops = native_hello(&load);
        ops.extend(native_hello(&update));
        rust_ops(ops)
    };
    let actual = ops(
        "hello_widget",
        &source,
        &context("hello_widget", "update", json!({}), state),
    );
    assert_eq!(actual, expected);
    assert!(
        source.len() < 3_000,
        "compiled source is {} bytes",
        source.len()
    );
}

fn native_hello(ctx: &Context) -> Vec<Op> {
    if ctx.event == "render" || ctx.event == "click" {
        let message =
            ctx.config_str("message").unwrap_or_default().into_owned();
        vec![Op::BarText { text: message }]
    } else {
        Vec::new()
    }
}

#[test]
fn activate_payload_matches_the_native_handler() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    match ctx.event.as_str() {
        "activate" => {
            let id = ctx.payload_str("id").unwrap_or_default();
            let prefix = ctx.config_str("message").unwrap_or_default();
            let status = format!("{}:{}", prefix, id);
            Ok(vec![Op::log(status), Op::state("service.status", json!({ "id": id }))])
        }
        _ => Ok(Vec::new()),
    }
}
"#;
    let source =
        compiled_source("activate_payload", handler, EntryKind::Service);
    let payload = json!({ "id": "timer-1" });
    let expected = rust_ops(native_activate(&context(
        "activate_payload",
        "activate",
        payload.clone(),
        json!({}),
    )));
    let actual = ops(
        "activate_payload",
        &source,
        &context("activate_payload", "activate", payload, json!({})),
    );
    assert_eq!(actual, expected);
}

fn native_activate(ctx: &Context) -> Vec<Op> {
    match ctx.event.as_str() {
        "activate" => {
            let id = ctx.payload_str("id").unwrap_or_default();
            let prefix = ctx.config_str("message").unwrap_or_default();
            let status = format!("{}:{}", prefix, id);
            vec![
                Op::log(status),
                Op::state("service.status", json!({ "id": id })),
            ]
        }
        _ => Vec::new(),
    }
}

#[test]
fn ui_tree_matches_the_native_handler() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let label = ctx.state_value::<String>("label").unwrap_or_default();
    let tree = ui::column(
        json!({ "id": "root", "gap": 4 }),
        vec![
            ui::label(json!({ "id": "title", "text": label })),
            ui::button(json!({ "id": "save", "label": "Save" })),
        ],
    );
    Ok(vec![Op::PanelRender { tree }])
}
"#;
    let source = compiled_source("ui_tree", handler, EntryKind::Panel);
    let state = json!({
        "label": "ready",
        "hello.state": { "label": "ready" }
    });

    let expected = {
        let load = context(
            "ui_tree",
            "render",
            json!({ "source": "load" }),
            state.clone(),
        );
        let update = context("ui_tree", "render", json!({}), state.clone());
        let mut ops = native_panel(&load);
        ops.extend(native_panel(&update));
        rust_ops(ops)
    };
    let actual = ops(
        "ui_tree",
        &source,
        &context("ui_tree", "update", json!({}), state),
    );
    assert_eq!(actual, expected);
}

fn native_panel(ctx: &Context) -> Vec<Op> {
    let label = ctx.state_value::<String>("label").unwrap_or_default();
    let tree = noctalia_rs::ui::column(
        json!({ "id": "root", "gap": 4 }),
        vec![
            noctalia_rs::ui::label(json!({ "id": "title", "text": label })),
            noctalia_rs::ui::button(json!({ "id": "save", "label": "Save" })),
        ],
    );
    vec![Op::PanelRender { tree }]
}

#[test]
fn callback_tables_match_the_bridge() {
    let bridge = noctalia_rs::codegen::CALLBACKS;
    let compiler = noctalia_rs_compiler::CALLBACKS;
    assert_eq!(bridge.len(), compiler.len());
    for (left, right) in bridge.iter().zip(compiler) {
        assert_eq!(left.event, right.event);
        assert_eq!(left.function, right.function);
        assert_eq!(left.params, right.params);
        assert_eq!(left.payload, right.payload);
    }
}

#[test]
fn early_return_compiles_and_runs() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event != "update" {
        return Ok(Vec::new());
    }
    Ok(vec![Op::log("tick".to_owned())])
}
"#;
    let source = compiled_source("early_return", handler, EntryKind::Service);
    let update = context("early_return", "update", json!({}), json!({}));
    let expected = rust_ops(native_early(&update));
    let actual = ops("early_return", &source, &update);
    assert_eq!(actual, expected);
}

fn native_early(ctx: &Context) -> Vec<Op> {
    if ctx.event != "update" {
        Vec::new()
    } else {
        vec![Op::log("tick")]
    }
}

#[test]
fn launcher_with_a_payload_default_arm_starts() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    match ctx.event.as_str() {
        "query" => {
            let text = ctx.payload_str("text").unwrap_or_default();
            Ok(vec![Op::LauncherResults { text, rows: vec![] }])
        }
        _ => {
            let source = ctx.payload_str("source").unwrap_or_default();
            Ok(vec![Op::log(source)])
        }
    }
}
"#;
    let source =
        compiled_source("launcher_payload", handler, EntryKind::Launcher);
    assert!(!source.contains("body_update(nil, \"load\")"));
    assert!(source.contains("body_query(last_query"));
    let update = context(
        "launcher_payload",
        "update",
        json!({ "source": "update" }),
        json!({}),
    );
    let expected = rust_ops(native_launcher(&update));
    let actual = ops("launcher_payload", &source, &update);
    assert_eq!(actual, expected);
}

fn native_launcher(ctx: &Context) -> Vec<Op> {
    match ctx.event.as_str() {
        "query" => {
            let text = ctx.payload_str("text").unwrap_or_default();
            vec![Op::LauncherResults {
                text,
                rows: Vec::new(),
            }]
        }
        _ => {
            let source = ctx.payload_str("source").unwrap_or_default();
            vec![Op::log(source)]
        }
    }
}

#[test]
fn inlined_module_helpers_match_the_native_handler() {
    let source = r#"
mod widget;
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    widget::handle(ctx)
}
"#;
    let widget = r#"
use noctalia_rs::{Context, Op, Result};

fn ensure(ctx: &Context, ops: &mut Vec<Op>) {
    if ctx.event == "render" {
        ops.push(Op::BarText { text: ctx.config_str("message").unwrap_or_default().into_owned() });
    }
}

fn label(ctx: &Context) -> String {
    format!("event={}", ctx.event)
}

pub fn handle(ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    ensure(ctx, &mut ops);
    ops.push(Op::log(label(ctx)));
    Ok(ops)
}
"#;
    let modules = [noctalia_rs_compiler::Module {
        path: "widget",
        source: widget,
    }];
    let compiled = compile(
        source,
        &Options {
            handler: "handler",
            entry: "inlined",
            state_key: "hello.state",
            kind: EntryKind::Widget,
            strings: &["message"],
            settings: &["message"],
            module: "main",
            modules: &modules,
            native: noctalia_rs_compiler::NativeMode::Deny,
            strict: None,
        },
    )
    .expect("compile")
    .source;
    let ctx = context("inlined", "render", json!({}), json!({}));
    let mut expected = rust_ops(vec![
        Op::BarText {
            text: "Hello".to_owned(),
        },
        Op::log("event=render"),
    ]);
    expected.extend(expected.clone());
    assert_eq!(ops("inlined", &compiled, &ctx), expected);
}

#[test]
fn unconditional_handler_dispatches_the_real_event() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::log(ctx.event.to_owned())])
}
"#;
    let source = compiled_source("runtime_event", handler, EntryKind::Widget);
    let update = context("runtime_event", "render", json!({}), json!({}));
    let click = context("runtime_event", "click", json!({}), json!({}));
    noctalia_rs_luau::dispatch(&update, &[("runtime_event", &source)])
        .expect("warm vm");
    assert_eq!(
        ops("runtime_event", &source, &update),
        rust_ops(vec![Op::log("render")])
    );
    assert_eq!(
        ops("runtime_event", &source, &click),
        rust_ops(vec![Op::log("click")])
    );
}

#[test]
fn default_branches_log_for_non_matching_events() {
    for (name, condition) in [
        ("positive_default", "ctx.event == \"click\""),
        ("negative_default", "ctx.event != \"click\""),
    ] {
        let handler = format!(
            "fn handler(ctx: &Context) -> Result<Vec<Op>> {{\n    if {condition} {{\n        Ok(vec![Op::log(\"A\")])\n    }} else {{\n        Ok(vec![Op::log(\"B\")])\n    }}\n}}\n"
        );
        let source = compiled_source(name, &handler, EntryKind::Widget);
        let click_ctx = context(name, "click", json!({}), json!({}));
        noctalia_rs_luau::dispatch(&click_ctx, &[(name, &source)])
            .expect("warm vm");
        let hover_ctx = context(name, "hover", json!({}), json!({}));
        let hover = ops(name, &source, &hover_ctx);
        let click = ops(name, &source, &click_ctx);
        let first = if condition.contains("!=") { "A" } else { "B" };
        assert_eq!(hover, rust_ops(vec![Op::log(first)]), "{name} hover");
        assert_eq!(
            click,
            rust_ops(vec![Op::log(if first == "A" { "B" } else { "A" })]),
            "{name} click"
        );
    }
}

fn every_op(ops: &[Op]) -> Vec<Value> {
    ops.iter()
        .map(|op| serde_json::to_value(op).expect("serialize"))
        .collect()
}

fn all_ops(entry: &str, source: &str, ctx: &Context) -> Vec<Value> {
    noctalia_rs_luau::dispatch(ctx, &[(entry, source)])
        .expect("dispatch")
        .iter()
        .map(|op| serde_json::to_value(op).expect("serialize"))
        .collect()
}

#[test]
fn stream_callback_ops_match_the_native_handler() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    if ctx.event == "load" {
        ops.push(Op::run_stream(json!(["tail", "-f"]), "stream"));
        ops.push(Op::state_watch("key", "watch"));
    }
    if ctx.event == "host_callback" && ctx.payload_str("id").as_deref() == Some("stream") {
        let line = ctx.payload_arg(0).and_then(Value::as_str).unwrap_or_default();
        ops.push(Op::state("line", json!(line)));
    }
    Ok(ops)
}
"#;
    let source =
        compiled_source("stream_callback", handler, EntryKind::Service);
    let load = context(
        "stream_callback",
        "load",
        json!({ "source": "load" }),
        json!({}),
    );
    let expected = every_op(&native_stream(&load));
    let actual = all_ops("stream_callback", &source, &load);
    assert_eq!(actual, expected);
    assert_eq!(actual[0]["path"], json!(["runStream"]));

    let callback = context(
        "stream_callback",
        "host_callback",
        json!({ "id": "stream", "args": ["hello"] }),
        json!({}),
    );
    let expected = every_op(&native_stream(&callback));
    let actual = all_ops("stream_callback", &source, &callback);
    assert_eq!(actual, expected);
    assert_eq!(actual[0]["value"], "hello");
}

fn native_stream(ctx: &Context) -> Vec<Op> {
    let mut ops = Vec::new();
    if ctx.event == "load" {
        ops.push(Op::run_stream(json!(["tail", "-f"]), "stream"));
        ops.push(Op::state_watch("key", "watch"));
    }
    if ctx.event == "host_callback"
        && ctx.payload_str("id").as_deref() == Some("stream")
    {
        let line = ctx
            .payload_arg(0)
            .and_then(Value::as_str)
            .unwrap_or_default();
        ops.push(Op::state("line", json!(line)));
    }
    ops
}

fn luau_parses(source: &str) -> Result<(), String> {
    mlua::Lua::new()
        .load(source)
        .into_function()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[test]
fn statement_heavy_handlers_emit_parseable_luau() {
    let handler = r#"
const LEVEL_AUTO: &str = "auto";
const LEVEL_ERROR: &str = "error";
fn render(ctx: &Context, ops: &mut Vec<Op>) {
    let label = ctx.payload_str("label").unwrap_or_default();
    ops.push(Op::set_interval(1000));
    let _ = label;
}
fn init(_ctx: &Context, ops: &mut Vec<Op>) {
    if ops.is_empty() {
        return;
    }
    ops.push(Op::log("init"));
}
fn checked(_ctx: &Context, ops: &mut Vec<Op>) -> bool {
    if ops.is_empty() {
        return true;
    }
    ops.push(Op::log("checked"));
    false
}
static INIT: OnceLock<()> = OnceLock::new();
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    let _ = INIT.get_or_init(|| init(ctx, &mut ops));
    let level = ctx.payload_str("level").unwrap_or_default();
    let text = match level.as_str() {
        value if value == LEVEL_ERROR => "error".to_owned(),
        value if matches!(value, LEVEL_AUTO | "manual") => "auto".to_owned(),
        _ => "other".to_owned(),
    };
    if text.is_empty() || checked(ctx, &mut ops) {
        return Ok(ops);
    }
    render(ctx, &mut ops);
    let values = vec![Op::log(text.clone())];
    ops.extend(values);
    drop(ops);
    Ok(vec![Op::log("done".to_owned())])
}
"#;
    let source =
        compiled_source("statement_heavy", handler, EntryKind::Service);
    luau_parses(&source).unwrap_or_else(|error| panic!("{error}\n{source}"));
}

#[test]
fn chained_conditions_emit_parseable_luau() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.is_load_event() {
        Ok(vec![Op::log("load")])
    } else if ctx.event == "render" {
        Ok(vec![Op::log("render")])
    } else if ctx.dark_mode() {
        Ok(vec![Op::log("dark")])
    } else {
        Ok(Vec::new())
    }
}
"#;
    let source = compiled_source("chained", handler, EntryKind::Widget);
    luau_parses(&source).unwrap_or_else(|error| panic!("{error}\n{source}"));
}

#[test]
fn json_value_breadth_matches_the_native_handler() {
    let handler = r#"
use serde_json::Map;

fn describe(value: Value) -> String {
    let kind = if value.is_null() {
        "null"
    } else if value.is_string() {
        "string"
    } else if value.is_number() {
        "number"
    } else if value.is_boolean() {
        "boolean"
    } else if value.is_array() {
        "array"
    } else {
        "other"
    };
    format!("{kind}:{}", value.to_string())
}

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    let mut props = Map::new();
    props.insert("text".to_owned(), json!("hi"));
    props.insert("count".to_owned(), json!(2));
    props.insert("flag".to_owned(), json!(true));
    if !props.contains_key("color") {
        props.insert("color".to_owned(), json!("red"));
    }
    let mut object = Value::Object(props);
    object["extra"] = json!([1, 2]);
    let text = object
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let count = object["count"].as_i64().unwrap_or_default();
    let ratio = object["count"].as_f64().unwrap_or(0.5);
    let missing = object.get("missing");
    let absent = missing.is_none() && json!(null).is_null();
    let flag = object["flag"].as_bool().unwrap_or(false);
    let array = object["extra"].as_array().map(|items| items.len()).unwrap_or(0);
    let object_keys = object.as_object().map(|map| map.len()).unwrap_or(0);
    let decoded = serde_json::from_str::<Value>(&object.to_string()).unwrap_or(Value::Null);
    let round = decoded["extra"].as_array().map(|items| items.len()).unwrap_or(0);
    let mut store = Map::new();
    store.entry("hits".to_owned()).or_insert(json!(0));
    store.entry("hits".to_owned()).or_insert(json!(9));
    let hits = store.get("hits").and_then(Value::as_i64).unwrap_or_default();
    let mut cloned = store.clone();
    cloned.insert("extra".to_owned(), json!("kept"));
    let original = store.len();
    let mut taken = Some(json!(4));
    let removed = taken.take().unwrap_or(Value::Null);
    let removed_number = removed.as_u64().unwrap_or_default();
    ops.push(Op::log(format!(
        "{text}|{count}|{ratio}|{absent}|{flag}|{array}|{object_keys}|{round}|{hits}|{original}|{removed_number}"
    )));
    ops.push(Op::log(describe(json!(null))));
    ops.push(Op::log(describe(json!("s"))));
    ops.push(Op::log(describe(json!(7))));
    ops.push(Op::log(describe(json!([1]))));
    ops.push(Op::log(describe(json!({ "a": 1 }))));
    Ok(ops)
}
"#;
    let source = compiled_source("json_breadth", handler, EntryKind::Service);
    let state = json!({});
    let ctx = context("json_breadth", "load", json!({}), state);
    assert_eq!(
        ops("json_breadth", &source, &ctx),
        rust_ops(native_json(&ctx))
    );
}

fn native_json(_ctx: &Context) -> Vec<Op> {
    use serde_json::Map;

    fn describe(value: Value) -> String {
        let kind = if value.is_null() {
            "null"
        } else if value.is_string() {
            "string"
        } else if value.is_number() {
            "number"
        } else if value.is_boolean() {
            "boolean"
        } else if value.is_array() {
            "array"
        } else {
            "other"
        };
        format!("{kind}:{value}")
    }

    let mut ops = Vec::new();
    let mut props = Map::new();
    props.insert("text".to_owned(), json!("hi"));
    props.insert("count".to_owned(), json!(2));
    props.insert("flag".to_owned(), json!(true));
    if !props.contains_key("color") {
        props.insert("color".to_owned(), json!("red"));
    }
    let mut object = Value::Object(props);
    object["extra"] = json!([1, 2]);
    let text = object
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let count = object["count"].as_i64().unwrap_or_default();
    let ratio = object["count"].as_f64().unwrap_or(0.5);
    let missing = object.get("missing");
    let absent = missing.is_none() && json!(null).is_null();
    let flag = object["flag"].as_bool().unwrap_or(false);
    let array = object["extra"]
        .as_array()
        .map(|items| items.len())
        .unwrap_or(0);
    let object_keys = object.as_object().map(|map| map.len()).unwrap_or(0);
    let decoded = serde_json::from_str::<Value>(&object.to_string())
        .unwrap_or(Value::Null);
    let round = decoded["extra"]
        .as_array()
        .map(|items| items.len())
        .unwrap_or(0);
    let mut store = Map::new();
    store.entry("hits".to_owned()).or_insert(json!(0));
    store.entry("hits".to_owned()).or_insert(json!(9));
    let hits = store
        .get("hits")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let mut cloned = store.clone();
    cloned.insert("extra".to_owned(), json!("kept"));
    let original = store.len();
    let mut taken = Some(json!(4));
    let removed = taken.take().unwrap_or(Value::Null);
    let removed_number = removed.as_u64().unwrap_or_default();
    ops.push(Op::log(format!(
        "{text}|{count}|{ratio}|{absent}|{flag}|{array}|{object_keys}|{round}|{hits}|{original}|{removed_number}"
    )));
    ops.push(Op::log(describe(json!(null))));
    ops.push(Op::log(describe(json!("s"))));
    ops.push(Op::log(describe(json!(7))));
    ops.push(Op::log(describe(json!([1]))));
    ops.push(Op::log(describe(json!({ "a": 1 }))));
    ops
}

#[test]
fn json_literals_match_the_native_handler() {
    let handler = r#"
use serde_json::Map;

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    let empty = json!([]);
    let shaped = json!({ "items": [], "meta": {} });
    let text = json!("hi").to_string();
    let number = json!(1.5).to_string();
    let integer = json!(3).to_string();
    let array = empty.as_array().map(|items| items.len()).unwrap_or(9);
    let array_kind = empty.is_array();
    let object_kind = shaped.is_object();
    let missing = shaped["items"].as_array().map(|items| items.len()).unwrap_or(9);
    let empty_map = Map::new();
    let is_empty = empty_map.is_empty();
    let mut filled = Map::new();
    filled.insert("b".to_owned(), json!(2));
    filled.insert("a".to_owned(), json!(1));
    let keys = filled.keys().next().unwrap_or_default();
    let first_value = filled.values().next().unwrap_or(Value::Null);
    let previous = filled.insert("a".to_owned(), json!(3));
    let previous_text = previous.as_ref().map(|value| value.to_string()).unwrap_or_default();
    let previous_some = previous.is_some();
    let mut ordered = Vec::new();
    for name in filled.keys() {
        ordered.push(name);
    }
    let dotted = format!("{}|{}|{}|{}", json!("x"), json!(1.5), json!([1]), json!({ "a": 1 }));
    ops.push(Op::log(format!(
        "{text}|{number}|{integer}|{array}|{array_kind}|{object_kind}|{missing}|{is_empty}|{keys}|{first_value}|{previous_text}|{previous_some}|{}",
        ordered.join(",")
    )));
    ops.push(Op::log(dotted));
    ops.push(Op::log(shaped.to_string()));
    Ok(ops)
}
"#;
    let source = compiled_source("json_literals", handler, EntryKind::Service);
    assert!(
        source.contains(noctalia_rs_luau::ARRAY_MARKER),
        "the compiler and the runtime must agree on the array marker"
    );
    let state = json!({});
    let ctx = context("json_literals", "load", json!({}), state);
    assert_eq!(
        ops("json_literals", &source, &ctx),
        rust_ops(native_json_literals(&ctx))
    );
}

fn native_json_literals(_ctx: &Context) -> Vec<Op> {
    use serde_json::Map;

    let mut ops = Vec::new();
    let empty = json!([]);
    let shaped = json!({ "items": [], "meta": {} });
    let text = json!("hi").to_string();
    let number = json!(1.5).to_string();
    let integer = json!(3).to_string();
    let array = empty.as_array().map(|items| items.len()).unwrap_or(9);
    let array_kind = empty.is_array();
    let object_kind = shaped.is_object();
    let missing = shaped["items"]
        .as_array()
        .map(|items| items.len())
        .unwrap_or(9);
    let empty_map = Map::new();
    let is_empty = empty_map.is_empty();
    let mut filled = Map::new();
    filled.insert("b".to_owned(), json!(2));
    filled.insert("a".to_owned(), json!(1));
    let keys = filled.keys().next().cloned().unwrap_or_default();
    let first_value = filled.values().next().cloned().unwrap_or(Value::Null);
    let previous = filled.insert("a".to_owned(), json!(3));
    let previous_text = previous
        .as_ref()
        .map(|value| value.to_string())
        .unwrap_or_default();
    let previous_some = previous.is_some();
    let mut ordered = Vec::new();
    for name in filled.keys() {
        ordered.push(name.clone());
    }
    let dotted = format!(
        "{}|{}|{}|{}",
        json!("x"),
        json!(1.5),
        json!([1]),
        json!({ "a": 1 })
    );
    ops.push(Op::log(format!(
        "{text}|{number}|{integer}|{array}|{array_kind}|{object_kind}|{missing}|{is_empty}|{keys}|{first_value}|{previous_text}|{previous_some}|{}",
        ordered.join(",")
    )));
    ops.push(Op::log(dotted));
    ops.push(Op::log(shaped.to_string()));
    ops
}

#[test]
fn json_iterators_and_scalars_match_the_native_handler() {
    let handler = r#"
use serde_json::Map;

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    let mut labels: Vec<String> = Vec::new();
    let mut numbers: Vec<Value> = Vec::new();
    let mut map = Map::new();
    map.insert("b".to_owned(), json!(2));
    map.insert("a".to_owned(), json!(1));
    map.insert("flag".to_owned(), json!(false));
    labels.push(json!("one").to_string());
    labels.push(format!("{}", json!(2)));
    numbers.push(json!(1));
    numbers.push(json!(2));
    let mut keys = map.keys();
    let first = keys.next().unwrap_or_default();
    let second = keys.next().unwrap_or_default();
    let third = keys.next().unwrap_or_default();
    let exhausted = keys.next().unwrap_or_default();
    let flag = map.get("flag").and_then(Value::as_bool).unwrap_or(true);
    let fallback = map.get("missing").cloned().unwrap_or(json!(true));
    let ordered: Vec<Value> = map.values().take(2).cloned().collect();
    let mut rendered = Vec::new();
    for (name, value) in map.iter() {
        rendered.push(format!("{name}={value}"));
    }
    ops.push(Op::log(format!(
        "{}|{}|{}|{}|{}|{}|{flag}|{fallback}|{}|{}",
        labels.join(","),
        numbers.len(),
        first,
        second,
        third,
        exhausted,
        ordered.len(),
        rendered.join(",")
    )));
    Ok(ops)
}
"#;
    let source = compiled_source("json_iterators", handler, EntryKind::Service);
    let state = json!({});
    let ctx = context("json_iterators", "load", json!({}), state);
    assert_eq!(
        ops("json_iterators", &source, &ctx),
        rust_ops(native_json_iterators(&ctx))
    );
}

fn native_json_iterators(_ctx: &Context) -> Vec<Op> {
    use serde_json::Map;

    let mut ops = Vec::new();
    let mut labels: Vec<String> = Vec::new();
    let mut numbers: Vec<Value> = Vec::new();
    let mut map = Map::new();
    map.insert("b".to_owned(), json!(2));
    map.insert("a".to_owned(), json!(1));
    map.insert("flag".to_owned(), json!(false));
    labels.push(json!("one").to_string());
    labels.push(format!("{}", json!(2)));
    numbers.push(json!(1));
    numbers.push(json!(2));
    let mut keys = map.keys();
    let first = keys.next().cloned().unwrap_or_default();
    let second = keys.next().cloned().unwrap_or_default();
    let third = keys.next().cloned().unwrap_or_default();
    let exhausted = keys.next().cloned().unwrap_or_default();
    let flag = map.get("flag").and_then(Value::as_bool).unwrap_or(true);
    let fallback = map.get("missing").cloned().unwrap_or(json!(true));
    let ordered: Vec<Value> = map.values().take(2).cloned().collect();
    let mut rendered = Vec::new();
    for (name, value) in map.iter() {
        rendered.push(format!("{name}={value}"));
    }
    ops.push(Op::log(format!(
        "{}|{}|{}|{}|{}|{}|{flag}|{fallback}|{}|{}",
        labels.join(","),
        numbers.len(),
        first,
        second,
        third,
        exhausted,
        ordered.len(),
        rendered.join(",")
    )));
    ops
}

#[test]
fn json_null_flags_and_map_chains_match_the_native_handler() {
    let handler = r#"
use serde_json::Map;

fn put(map: &mut Map, value: Value) {
    map.insert("v".to_owned(), value);
}

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    let mut map = Map::new();
    map.insert("b".to_owned(), json!(2));
    map.insert("a".to_owned(), json!(1));
    let keys: Vec<String> = map.keys().cloned().collect();
    let pair_keys: Vec<String> = map.iter().map(|(key, _)| key.clone()).collect();
    let vals: Vec<Value> = map.values().take(1).cloned().collect();
    let filtered: Vec<String> = map
        .iter()
        .filter(|(_, value)| value.as_i64().unwrap_or(0) > 1)
        .map(|(key, _)| key.clone())
        .collect();
    let skipped: Vec<String> = map.keys().skip(1).cloned().collect();
    let mut plain = Vec::new();
    for (key, value) in &map {
        plain.push(format!("{key}={value}"));
    }
    let one: Vec<String> = map.keys().cloned().collect();
    let flag = json!(false).as_bool().unwrap_or(true);
    ops.push(Op::log(format!(
        "{}|{}|{}|{}|{}|{}|{}|{flag}",
        keys.join(","),
        pair_keys.join(","),
        vals.len(),
        filtered.join(","),
        skipped.join(","),
        plain.join(","),
        one.join(",")
    )));
    Ok(ops)
}
"#;
    let source = compiled_source("json_chains", handler, EntryKind::Service);
    assert!(source.contains(noctalia_rs_luau::ARRAY_MARKER));
    let state = json!({});
    let ctx = context("json_chains", "load", json!({}), state);
    assert_eq!(
        ops("json_chains", &source, &ctx),
        rust_ops(native_json_chains(&ctx))
    );
}

fn native_json_chains(_ctx: &Context) -> Vec<Op> {
    use serde_json::Map;

    let mut ops = Vec::new();
    let mut map = Map::new();
    map.insert("b".to_owned(), json!(2));
    map.insert("a".to_owned(), json!(1));
    let keys: Vec<String> = map.keys().cloned().collect();
    let pair_keys: Vec<String> =
        map.iter().map(|(key, _)| key.clone()).collect();
    let vals: Vec<Value> = map.values().take(1).cloned().collect();
    let filtered: Vec<String> = map
        .iter()
        .filter(|(_, value)| value.as_i64().unwrap_or(0) > 1)
        .map(|(key, _)| key.clone())
        .collect();
    let skipped: Vec<String> = map.keys().skip(1).cloned().collect();
    let mut plain = Vec::new();
    for (key, value) in &map {
        plain.push(format!("{key}={value}"));
    }
    let one: Vec<String> = map.keys().cloned().collect();
    let flag = json!(false).as_bool().unwrap_or(true);
    ops.push(Op::log(format!(
        "{}|{}|{}|{}|{}|{}|{}|{flag}",
        keys.join(","),
        pair_keys.join(","),
        vals.len(),
        filtered.join(","),
        skipped.join(","),
        plain.join(","),
        one.join(",")
    )));
    ops
}

#[test]
fn newtype_and_nested_values_reach_the_host_when_non_null() {
    let handler = r#"
struct Holder(Value);

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let nested = ((json!("a"), json!(2)), json!(3));
    let (inner, _) = nested;
    let (_, name) = inner;
    let holder = Holder(name);
    let mut map = serde_json::Map::new();
    map.insert("k".to_owned(), holder.0);
    let value = ctx.setting("flag").unwrap_or(Value::Null);
    ops_push(value.is_null());
    Ok(vec![Op::state("seen", json!(map))])
}

fn ops_push(_flag: bool) {}
"#;
    let source = compiled_source("newtype_chain", handler, EntryKind::Service);
    let state = json!({});
    let ctx = context("newtype_chain", "load", json!({}), state);
    assert_eq!(
        ops("newtype_chain", &source, &ctx),
        rust_ops(native_newtype_chain(&ctx))
    );
}

fn native_newtype_chain(_ctx: &Context) -> Vec<Op> {
    use serde_json::Map;

    let nested = ((json!("a"), json!(2)), json!(3));
    let (inner, _) = nested;
    let (_, name) = inner;
    let mut map = Map::new();
    map.insert("k".to_owned(), name);
    vec![Op::state("seen", json!(map))]
}

#[test]
fn tuple_fields_helpers_and_newtypes_reach_the_host_when_non_null() {
    let handler = r#"
struct Holder { pair: (Value, Value) }
struct Inner(Value);
struct Outer(Inner);

fn make() -> (Value, Value) { (json!("a"), json!(2)) }

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let holder = Holder { pair: (json!("field"), json!(1)) };
    let (_, from_field) = holder.pair;
    let (_, from_helper) = make();
    let outer = Outer(Inner(json!("newtype")));
    let value = ctx.setting("flag").unwrap_or(Value::Null);
    let _ = value;
    Ok(vec![Op::state("field", json!(from_field)),
            Op::state("helper", json!(from_helper)),
            Op::state("newtype", json!(outer.0.0))])
}
"#;
    let source = compiled_source("tuple_paths", handler, EntryKind::Service);
    let state = json!({});
    let ctx = context("tuple_paths", "load", json!({}), state);
    assert_eq!(
        ops("tuple_paths", &source, &ctx),
        rust_ops(native_tuple_paths(&ctx))
    );
}

fn native_tuple_paths(_ctx: &Context) -> Vec<Op> {
    vec![
        Op::state("field", json!(1)),
        Op::state("helper", json!(2)),
        Op::state("newtype", json!("newtype")),
    ]
}

#[test]
fn round_eight_tuple_paths_reach_the_host_when_non_null() {
    let handler = r#"
struct Holder { pair: (Value, Value) }
struct Pair(Value, Value);
fn make() -> (Value, Value) { let pair = (json!(1), json!("a")); pair }

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let (_, from_helper) = make();
    let holder = Holder { pair: (json!(5), json!(6)) };
    let (_, from_field) = holder.pair;
    let pair = Pair(json!(2), json!("b"));
    Ok(vec![Op::state("helper", json!(from_helper)),
            Op::state("field", json!(from_field)),
            Op::state("pair", json!(pair.1))])
}
"#;
    let source = compiled_source("tuple_round8", handler, EntryKind::Service);
    let ctx = context("tuple_round8", "load", json!({}), json!({}));
    assert_eq!(
        ops("tuple_round8", &source, &ctx),
        rust_ops(native_tuple_round8(&ctx))
    );
}

fn native_tuple_round8(_ctx: &Context) -> Vec<Op> {
    vec![
        Op::state("helper", json!("a")),
        Op::state("field", json!(6)),
        Op::state("pair", json!("b")),
    ]
}

#[test]
fn round_nine_chains_reach_the_host_when_non_null() {
    let handler = r#"
struct Holder { pair: (Value, Value) }
fn make() -> (Value, Value) { (json!(1), json!("a")) }
fn passthrough(t: (Value, Value)) -> (Value, Value) { t }
static PAIR: (Value, Value) = (json!(2), json!("s"));

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let t = make();
    let (_, from_local) = t;
    let (_, from_helper) = passthrough(make());
    let mut holder = Holder { pair: make() };
    holder = Holder { pair: (json!(3), json!(4)) };
    let (_, from_field) = holder.pair;
    let (_, from_static) = PAIR;
    Ok(vec![Op::state("local", json!(from_local)),
            Op::state("helper", json!(from_helper)),
            Op::state("field", json!(from_field)),
            Op::state("static", json!(from_static))])
}
"#;
    let source = compiled_source("chain9", handler, EntryKind::Service);
    let ctx = context("chain9", "load", json!({}), json!({}));
    assert_eq!(ops("chain9", &source, &ctx), rust_ops(native_chain9(&ctx)));
}

fn native_chain9(_ctx: &Context) -> Vec<Op> {
    vec![
        Op::state("local", json!("a")),
        Op::state("helper", json!("a")),
        Op::state("field", json!(4)),
        Op::state("static", json!("s")),
    ]
}

#[test]
fn round_ten_chains_reach_the_host_when_non_null() {
    let handler = r#"
fn passthrough(t: (Value, Value)) -> (Value, Value) { t }
fn make_pair() -> (Value, Value) { (json!(1), json!(2)) }
fn make_value() -> Value { json!(3) }

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let (_, from_arg) = passthrough((json!(1), json!(2)));
    let mut t = (json!(1), json!(2));
    t = make_pair();
    let (_, from_call) = t;
    static PAIR: (Value, Value) = (json!(4), make_value());
    let (_, from_static) = PAIR;
    Ok(vec![Op::state("arg", json!(from_arg)),
            Op::state("call", json!(from_call)),
            Op::state("static", json!(from_static))])
}
"#;
    let source = compiled_source("chain10", handler, EntryKind::Service);
    let ctx = context("chain10", "load", json!({}), json!({}));
    assert_eq!(
        ops("chain10", &source, &ctx),
        rust_ops(native_chain10(&ctx))
    );
}

fn native_chain10(_ctx: &Context) -> Vec<Op> {
    vec![
        Op::state("arg", json!(2)),
        Op::state("call", json!(2)),
        Op::state("static", json!(3)),
    ]
}

fn duplicated(ops: Vec<Op>) -> Vec<Value> {
    let values = rust_ops(ops);
    [values.clone(), values].concat()
}

#[test]
fn chars_iterator_keeps_utf8_boundaries() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let multibyte = "éab";
    let mut chars = multibyte.chars();
    let first = format!("{}{}", chars.next().unwrap_or(' '), chars.as_str());
    let rest = multibyte.chars().skip(1).as_str();
    let taken = multibyte.chars().take(2).collect::<String>();
    let full = multibyte.chars().collect::<String>();
    let empty = "".chars().collect::<String>();
    let ascii = "abc";
    let ascii_rest = ascii.chars().skip(1).as_str();
    Ok(vec![
        Op::BarText { text: format!("{first}|{rest}|{taken}|{full}|{empty}") },
        Op::BarText { text: format!("{ascii_rest}") },
    ])
}
"#;
    let source = compiled_source("chars", handler, EntryKind::Widget);
    let actual = ops(
        "chars",
        &source,
        &context("chars", "update", json!({}), json!({})),
    );
    let expected = duplicated(vec![
        Op::BarText {
            text: "éab|ab|éa|éab|".to_owned(),
        },
        Op::BarText {
            text: "bc".to_owned(),
        },
    ]);
    assert_eq!(actual, expected);
}

#[test]
fn keyword_and_plain_bindings_do_not_share_lua_names() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let end = 1;
    let __kw_end = 2;
    let first = format!("{end}|{__kw_end}");
    let nested = {
        let __kw_end = 4;
        let end = 3;
        format!("{end}|{__kw_end}")
    };
    let reversed = {
        let other_end = 5;
        let end = 6;
        format!("{end}|{other_end}")
    };
    Ok(vec![
        Op::BarText { text: first },
        Op::BarText { text: nested },
        Op::BarText { text: reversed },
    ])
}
"#;
    let source = compiled_source("names", handler, EntryKind::Widget);
    let actual = ops(
        "names",
        &source,
        &context("names", "update", json!({}), json!({})),
    );
    let expected = duplicated(vec![
        Op::BarText {
            text: "1|2".to_owned(),
        },
        Op::BarText {
            text: "3|4".to_owned(),
        },
        Op::BarText {
            text: "6|5".to_owned(),
        },
    ]);
    assert_eq!(actual, expected);
}

#[test]
fn chars_cursor_advances_once_after_next() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut iter = "é中ab".chars();
    iter.next();
    let skipped = iter.skip(1).as_str();
    let mut taken_iter = "é中ab".chars();
    taken_iter.next();
    let taken = taken_iter.take(1).collect::<String>();
    Ok(vec![Op::BarText { text: format!("{skipped}|{taken}") }])
}
"#;
    let source = compiled_source("chars_cursor", handler, EntryKind::Widget);
    let actual = ops(
        "chars_cursor",
        &source,
        &context("chars_cursor", "update", json!({}), json!({})),
    );
    assert_eq!(
        actual,
        duplicated(vec![Op::BarText {
            text: "ab|中".to_owned()
        }])
    );
}

#[test]
fn local_names_shadow_ui_helpers() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let callback = |value: &str| value.to_owned();
    let call = |value: &str| value.to_owned();
    let node = |value: &str| value.to_owned();
    let button = |value: &str| value.to_owned();
    Ok(vec![Op::BarText { text: format!(
        "{}|{}|{}|{}",
        callback("callback"), call("call"), node("node"), button("button")
    ) }])
}
"#;
    let source = compiled_source("locals", handler, EntryKind::Widget);
    let actual = ops(
        "locals",
        &source,
        &context("locals", "update", json!({}), json!({})),
    );
    let expected = duplicated(vec![Op::BarText {
        text: "\"callback\"|\"call\"|\"node\"|\"button\"".to_owned(),
    }]);
    assert_eq!(actual, expected);
}

#[test]
fn float_max_and_min_ignore_nan_like_rust() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let larger = f64::max(f64::NAN, 2.0);
    let smaller = f64::min(f64::NAN, 2.0);
    let integer = i64::max(3, 4);
    Ok(vec![Op::BarText { text: format!("{larger}|{smaller}|{integer}") }])
}
"#;
    let source = compiled_source("nan", handler, EntryKind::Widget);
    let actual = ops(
        "nan",
        &source,
        &context("nan", "update", json!({}), json!({})),
    );
    assert_eq!(
        actual,
        duplicated(vec![Op::BarText {
            text: "2|2|4".to_owned()
        }])
    );
}

#[test]
fn review_z_scoping_and_let_chains() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    const LIMIT: i64 = 1;
    let end = 1;
    {
        const LIMIT: i64 = 9;
        let _ = LIMIT;
    }
    let __kw_end = 2;
    let mut pair = (1, 2);
    (pair.0, pair.1) = (pair.1, pair.0);
    let hit = if let Some(a) = Some(4) && let Some(b) = Some(5) {
        format!("{a}{b}")
    } else {
        "miss".to_owned()
    };
    let miss = if let Some(c) = None && let Some(d) = Some(5) {
        format!("{c}{d}")
    } else {
        "miss".to_owned()
    };
    let first = pair.0;
    let second = pair.1;
    Ok(vec![Op::BarText { text: format!("{LIMIT}|{end}|{__kw_end}|{first}{second}|{hit}|{miss}") }])
}
"#;
    let source = compiled_source("zscope", handler, EntryKind::Widget);
    let actual = ops(
        "zscope",
        &source,
        &context("zscope", "update", json!({}), json!({})),
    );
    assert_eq!(
        actual,
        duplicated(vec![Op::BarText {
            text: "1|1|2|21|45|miss".to_owned()
        }])
    );
}

#[test]
fn review_z_match_guards_and_slices() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let numbers = [1, 2, 3];
    let slice = match numbers {
        [first, .., last] => format!("{first}{last}"),
        _ => String::new(),
    };
    let guarded = match 2 {
        value if value == 1 => "one".to_owned(),
        value if value == 2 => "two".to_owned(),
        _ => "other".to_owned(),
    };
    let empty: Option<i64> = None;
    let none = match empty {
        Some(value) => format!("{value}"),
        None => "none".to_owned(),
    };
    Ok(vec![Op::BarText { text: format!("{slice}|{guarded}|{none}") }])
}
"#;
    let source = compiled_source("zguard", handler, EntryKind::Widget);
    let actual = ops(
        "zguard",
        &source,
        &context("zguard", "update", json!({}), json!({})),
    );
    assert_eq!(
        actual,
        duplicated(vec![Op::BarText {
            text: "13|two|none".to_owned()
        }])
    );
}

#[test]
fn review_gg_short_circuit_scopes_and_match_fallbacks() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let n = 7;
    let mut total = 0;
    let hit = if let Some(a) = Some(1) && let Some(b) = Some(2) {
        total = total + 100;
        format!("{a}{b}")
    } else {
        "miss".to_owned()
    };
    let miss = if let Some(_) = None && let Some(c) = { total = total + 1; Some(b2()) } {
        format!("{c}")
    } else {
        format!("{n}|{total}")
    };
    let later = if let Some(x) = Some(3) && let Some(y) = Some(x + 1) {
        y
    } else {
        0
    };
    let iter = "éx".chars();
    {
        let iter = "block".to_owned();
        let _ = iter;
    }
    let first = iter.next();
    let rest = iter.as_str();
    let empty: Option<(i64,)> = None;
    let indexed = if let Some((value,)) = empty {
        format!("{value}")
    } else {
        "indexed-else".to_owned()
    };
    let outer = 9;
    let guard = match 2 {
        value if false => format!("{value}"),
        _ => format!("{outer}"),
    };
    let fallback = match 2 {
        1 if false => "one".to_owned(),
        _ => "fallback".to_owned(),
    };
    Ok(vec![Op::BarText { text: format!("{hit}|{miss}|{later}|{first}{rest}|{indexed}|{guard}|{fallback}") }])
}

fn b2() -> i64 {
    2
}
"#;
    let source = compiled_source("gg", handler, EntryKind::Widget);
    let actual = ops(
        "gg",
        &source,
        &context("gg", "update", json!({}), json!({})),
    );
    assert_eq!(
        actual,
        duplicated(vec![Op::BarText {
            text: "12|7|100|4|éx|indexed-else|9|fallback".to_owned()
        }])
    );
}

#[test]
fn review_nn_branch_scopes_types_and_guard_setup() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let n = 7;
    let picked = if true {
        let n = 9;
        n
    } else {
        0
    };
    let ratio = if let Some(x) = Some(5) && let Some(y) = Some(2) {
        x / y
    } else {
        0
    };
    let guard_pick = match 2 {
        n if { let accepted = n == 2; accepted } => 1,
        _ => 0,
    };
    Ok(vec![Op::BarText { text: format!("{picked}|{n}|{ratio}|{guard_pick}") }])
}
"#;
    let source = compiled_source("nn", handler, EntryKind::Widget);
    let actual = ops(
        "nn",
        &source,
        &context("nn", "update", json!({}), json!({})),
    );
    assert_eq!(
        actual,
        duplicated(vec![Op::BarText {
            text: "9|7|2|1".to_owned()
        }])
    );
}

#[test]
fn review_pp_branch_result_shapes_and_iterators() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let (a, b) = if true {
        (5, 2)
    } else {
        (0, 1)
    };
    let ratio = a / b;
    let chars = (if true {
        let mut iter = "ab".chars();
        iter.next();
        iter
    } else {
        "xy".chars()
    }).collect::<String>();
    Ok(vec![Op::BarText { text: format!("{ratio}|{chars}") }])
}
"#;
    let source = compiled_source("pp", handler, EntryKind::Widget);
    let actual = ops(
        "pp",
        &source,
        &context("pp", "update", json!({}), json!({})),
    );
    assert_eq!(
        actual,
        duplicated(vec![Op::BarText {
            text: "2|b".to_owned()
        }])
    );
}

#[test]
fn review_qq_void_if_branches_stay_parseable() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let flag = ctx.event == "never";
    let _picked = if flag {
        Op::log("a")
    } else {
        Op::log("b")
    };
    Ok(vec![Op::log("done")])
}
"#;
    let source = compiled_source("qq", handler, EntryKind::Widget);
    let actual = ops(
        "qq",
        &source,
        &context("qq", "update", json!({}), json!({})),
    );
    assert_eq!(
        actual,
        duplicated(vec![Op::log("b".to_owned()), Op::log("done".to_owned())])
    );
}

#[test]
fn review_l1_stable_sort_matches_rust() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut items = vec![
        (0_i64, 11_i64), (0, 10), (0, 9), (0, 8), (0, 7), (0, 6),
        (0, 5), (0, 4), (0, 3), (0, 2), (0, 1), (0, 0),
    ];
    items.sort_by_key(|item| item.0);
    let mut ops = Vec::new();
    for (_, id) in items {
        ops.push(Op::log(id.to_string()));
    }
    Ok(ops)
}
"#;
    let source = compiled_source("review_l1", handler, EntryKind::Service);
    let ctx = context("review_l1", "render", json!({}), json!({}));
    let luau = ops("review_l1", &source, &ctx);
    let native =
        duplicated((0..12).rev().map(|id| Op::log(id.to_string())).collect());
    assert_eq!(luau, native);
}

#[test]
fn review_l2_retain_matches_rust() {
    let handler = r#"
static LAST: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut numbers = vec![1_i64, 2, 3, 4];
    numbers.retain(|number| {
        LAST.store(*number, std::sync::atomic::Ordering::Relaxed);
        *number % 2 == 0
    });
    let mut ops = Vec::new();
    for number in numbers {
        ops.push(Op::log(number.to_string()));
    }
    ops.push(Op::log(LAST.load(std::sync::atomic::Ordering::Relaxed).to_string()));
    Ok(ops)
}
"#;
    let source = compiled_source("review_l2", handler, EntryKind::Service);
    let ctx = context("review_l2", "render", json!({}), json!({}));
    let luau = ops("review_l2", &source, &ctx);
    let native = duplicated(vec![
        Op::log("2".to_owned()),
        Op::log("4".to_owned()),
        Op::log("4".to_owned()),
    ]);
    assert_eq!(luau, native);
}

#[test]
fn review_l3_bounded_range_matches_rust() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let values: Vec<i64> = (1..4).collect();
    let mut ops = Vec::new();
    for value in values {
        ops.push(Op::log(value.to_string()));
    }
    Ok(ops)
}
"#;
    let source = compiled_source("review_l3", handler, EntryKind::Service);
    let ctx = context("review_l3", "render", json!({}), json!({}));
    let luau = ops("review_l3", &source, &ctx);
    let native = duplicated(vec![
        Op::log("1".to_owned()),
        Op::log("2".to_owned()),
        Op::log("3".to_owned()),
    ]);
    assert_eq!(luau, native);
}

#[test]
fn review_l4_replacen_matches_rust() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let text = ctx.config_str("message").unwrap_or_default();
    Ok(vec![Op::log(text.replacen("e", "E", 1))])
}
"#;
    let source = compiled_source("review_l4", handler, EntryKind::Service);
    let ctx = context("review_l4", "render", json!({}), json!({}));
    let luau = ops("review_l4", &source, &ctx);
    let native = duplicated(vec![Op::log("HEllo".to_owned())]);
    assert_eq!(luau, native);
}

#[test]
fn review_l5_boundary_safe_truncate_matches_rust() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut text = "abcd".to_owned();
    text.truncate(2);
    Ok(vec![Op::log(text)])
}
"#;
    let source = compiled_source("review_l5", handler, EntryKind::Service);
    let ctx = context("review_l5", "render", json!({}), json!({}));
    let luau = ops("review_l5", &source, &ctx);
    let native = duplicated(vec![Op::log("ab".to_owned())]);
    assert_eq!(luau, native);
}

#[test]
fn review_l6_wrapping_add_matches_rust() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = i64::MAX.wrapping_add(1);
    Ok(vec![Op::log(if value == i64::MIN { "wrapped" } else { "wrong" })])
}
"#;
    let source = compiled_source("review_l6", handler, EntryKind::Service);
    let ctx = context("review_l6", "render", json!({}), json!({}));
    let luau = ops("review_l6", &source, &ctx);
    let native = duplicated(vec![Op::log("wrapped".to_owned())]);
    assert_eq!(luau, native);
}

#[test]
fn review_l7_direct_static_cell_matches_rust() {
    let handler = r#"
static CELL: OnceLock<i64> = OnceLock::new();
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = *CELL.get_or_init(|| 3);
    Ok(vec![Op::log(value.to_string())])
}
"#;
    let source = compiled_source("review_l7", handler, EntryKind::Service);
    let ctx = context("review_l7", "render", json!({}), json!({}));
    let luau = ops("review_l7", &source, &ctx);
    let native = duplicated(vec![Op::log("3".to_owned())]);
    assert_eq!(luau, native);
}

#[test]
fn review_l8_ascii_lowercase_matches_rust() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let text = "ABCD".to_lowercase();
    Ok(vec![Op::log(text)])
}
"#;
    let source = compiled_source("review_l8", handler, EntryKind::Service);
    let ctx = context("review_l8", "render", json!({}), json!({}));
    let luau = ops("review_l8", &source, &ctx);
    let native = duplicated(vec![Op::log("abcd".to_owned())]);
    assert_eq!(luau, native);
}

#[test]
fn review_l9_max_ignores_nan_like_rust() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = f64::NAN.max(2.0);
    Ok(vec![Op::log(if value == 2.0 { "ok" } else { "bad" })])
}
"#;
    let source = compiled_source("review_l9", handler, EntryKind::Service);
    let ctx = context("review_l9", "render", json!({}), json!({}));
    let luau = ops("review_l9", &source, &ctx);
    let native = duplicated(vec![Op::log("ok".to_owned())]);
    assert_eq!(luau, native);
}

#[test]
fn review_l10_parse_and_fallback_match_rust() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let good = "42".parse::<i64>().unwrap_or(0);
    let bad = "abc".parse::<i64>().unwrap_or(7);
    Ok(vec![Op::log(format!("{}:{}", good, bad))])
}
"#;
    let source = compiled_source("review_l10", handler, EntryKind::Service);
    let ctx = context("review_l10", "render", json!({}), json!({}));
    let luau = ops("review_l10", &source, &ctx);
    let native = duplicated(vec![Op::log("42:7".to_owned())]);
    assert_eq!(luau, native);
}

#[test]
fn review_l7_alias_cell_matches_rust() {
    let handler = r#"
static CELL: OnceLock<i64> = OnceLock::new();
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let slot = &CELL;
    let value = slot.get_or_init(|| 3);
    Ok(vec![Op::log(value.to_string())])
}
"#;
    let source =
        compiled_source("review_l7_alias", handler, EntryKind::Service);
    let ctx = context("review_l7_alias", "render", json!({}), json!({}));
    let luau = ops("review_l7_alias", &source, &ctx);
    let native = duplicated(vec![Op::log("3".to_owned())]);
    assert_eq!(luau, native);
}

#[test]
fn review_round3_width_and_parse_match_rust() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let a = u8::MAX.saturating_add(1);
    let b = u32::MAX.wrapping_add(1);
    let c = "0000000000000000042".parse::<i64>().unwrap_or(0);
    let d = "1e2".parse::<f64>().unwrap_or(0.0);
    Ok(vec![Op::log(format!("{} {} {} {}", a, b, c, d))])
}
"#;
    let source = compiled_source("review_round3", handler, EntryKind::Service);
    let ctx = context("review_round3", "render", json!({}), json!({}));
    let luau = ops("review_round3", &source, &ctx);
    let native = duplicated(vec![Op::log("255 0 42 100".to_owned())]);
    assert_eq!(luau, native);
}

#[test]
fn review_round4_dispatch_checks() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let a = "42".parse::<f64>().unwrap_or(7.0);
    let b = "1.5e2".parse::<f64>().unwrap_or(0.0);
    let c = 20_u8.saturating_mul(20);
    let found = vec![vec![10_i64], vec![20]].into_iter().flatten().find(|x| **x == 10);
    let nth = vec![vec![10_i64], vec![20]].into_iter().flatten().nth(0);
    let taken = vec![1_i64, 2, 3].into_iter().take_while(|x| **x < 3).collect::<Vec<_>>();
    let reversed = vec![2_i64, 3, 4].into_iter().rev().collect::<Vec<_>>();
    Ok(vec![Op::log(format!("{} {} {} {} {} {} {}", a, b, c, found.unwrap_or(0), nth.unwrap_or(0), taken.len(), reversed[0]))])
}
"#;
    let source = compiled_source("review_round4", handler, EntryKind::Service);
    let ctx = context("review_round4", "render", json!({}), json!({}));
    let luau = ops("review_round4", &source, &ctx);
    let native = duplicated(vec![Op::log("42 150 255 10 10 2 4".to_owned())]);
    assert_eq!(luau, native);
}

#[test]
fn review_round6_enumerate_and_nested_take_while() {
    let handler = r#"
static SEEN: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    SEEN.store(0, std::sync::atomic::Ordering::Relaxed);
    let ratio = [1.5_f64].into_iter().enumerate().map(|(_, x)| x / (x + x)).collect::<Vec<_>>();
    let values = vec![1_i64, 2, 3, 4].into_iter().filter(|x| {
        SEEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        *x < 4
    }).take_while(|x| **x < 4).collect::<Vec<_>>();
    Ok(vec![Op::log(format!("{} {} {}", ratio[0], values.len(), SEEN.load(std::sync::atomic::Ordering::Relaxed)))])
}
"#;
    let source = compiled_source("review_round6", handler, EntryKind::Service);
    let ctx = context("review_round6", "render", json!({}), json!({}));
    eprintln!("SOURCE6:\n{source}");
    let luau = ops("review_round6", &source, &ctx);
    let native = duplicated(vec![Op::log("0.5 3 4".to_owned())]);
    assert_eq!(luau, native);
}

#[test]
fn review_round7_zip_types_and_nested_take_while() {
    let handler = r#"
static SEEN: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    SEEN.store(0, std::sync::atomic::Ordering::Relaxed);
    let ratio = [9_i64].into_iter().zip([4_i64].into_iter())
        .map(|(a, b)| a / b).collect::<Vec<_>>();
    let values = vec![1_i64, 2, 3, 4].into_iter().filter(|x| {
        SEEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        *x < 4
    }).take_while(|_| true).take_while(|x| **x < 3).collect::<Vec<_>>();
    Ok(vec![Op::log(format!("{} {} {}", ratio[0], values.len(), SEEN.load(std::sync::atomic::Ordering::Relaxed)))])
}
"#;
    let source = compiled_source("review_round7", handler, EntryKind::Service);
    let ctx = context("review_round7", "render", json!({}), json!({}));
    let luau = ops("review_round7", &source, &ctx);
    let native = duplicated(vec![Op::log("2 2 3".to_owned())]);
    assert_eq!(luau, native);
}
