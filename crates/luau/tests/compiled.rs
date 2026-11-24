#![allow(clippy::too_many_arguments)]

use std::path::PathBuf;

use noctalia_rs::{Context, Op, Parts};
use noctalia_rs_compiler::{EntryKind, Options, compile};
use serde_json::{Value, json};

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
        source.len() < 900,
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
