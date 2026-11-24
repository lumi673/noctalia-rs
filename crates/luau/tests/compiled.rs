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
