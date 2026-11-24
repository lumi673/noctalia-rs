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
            module: "main",
            modules: &[],
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
            module: "main",
            modules: &modules,
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
