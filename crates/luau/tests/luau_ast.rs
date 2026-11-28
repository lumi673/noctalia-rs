//! Proof for the strict Luau AST pipeline and its semantic passes.

use std::path::PathBuf;

use noctalia_rs::{Context, Parts};
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
    .expect("compile strict handler")
    .source
}

fn auto_source(handler: &str, kind: EntryKind) -> String {
    let source = format!(
        "use noctalia_rs::{{Context, Op, Result}};\nuse serde_json::{{Value, json}};\n\n{handler}\n"
    );
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
            native: NativeMode::Allow,
            strict: None,
        },
    )
    .expect("compile auto handler")
    .source
}

fn luau_parses(source: &str) {
    mlua::Lua::new()
        .load(source)
        .set_name("generated.luau")
        .into_function()
        .expect("generated strict script parses");
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
            data_dir: PathBuf::from("/tmp/noctalia-luau-ast"),
        },
        json!({}),
    )
}

fn ops(source: &str, event: &str, payload: Value) -> Vec<Value> {
    noctalia_rs_luau::dispatch_strict(
        &context(event, payload),
        &[("strict", source)],
    )
    .expect("dispatch strict script")
    .iter()
    .map(|op| serde_json::to_value(op).expect("serialize op"))
    .collect()
}

fn message(values: &[Value]) -> Vec<String> {
    values
        .iter()
        .filter(|op| op["op"] == json!("log"))
        .map(|op| op["message"].as_str().unwrap_or_default().to_owned())
        .collect()
}

#[test]
fn constant_folding_emits_an_exact_literal() {
    let handler = r#"
const STEP: i64 = 20;

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = STEP + 22;
    Ok(vec![Op::log(value.to_string())])
}
"#;
    let source = strict_source(handler, EntryKind::Service);
    luau_parses(&source);
    assert!(source.contains("local value = 42"), "{source}");
    assert!(!source.contains("20 + 22"), "{source}");
    let values = message(&ops(&source, "update", json!({})));
    assert!(
        !values.is_empty() && values.iter().all(|value| value == "42"),
        "{values:?}"
    );

    let auto = auto_source(handler, EntryKind::Service);
    let auto_values = message(&ops(&auto, "update", json!({})));
    assert!(
        !auto_values.is_empty()
            && auto_values.iter().all(|value| value == "42"),
        "{auto_values:?}"
    );
}

#[test]
fn dead_code_removal_drops_a_false_branch() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    if false {
        return Ok(vec![Op::log("never")]);
    }
    Ok(vec![Op::log("live")])
}
"#;
    let source = strict_source(handler, EntryKind::Service);
    luau_parses(&source);
    assert!(!source.contains("never"), "{source}");
    let values = message(&ops(&source, "update", json!({})));
    assert!(
        !values.is_empty() && values.iter().all(|value| value == "live"),
        "{values:?}"
    );
}

#[test]
fn callback_pruning_drops_an_unreachable_dispatch_global() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    Ok(Vec::new())
}
"#;
    let source = strict_source(handler, EntryKind::Service);
    luau_parses(&source);
    assert!(!source.contains("function __strict_callback("), "{source}");
    assert!(ops(&source, "host_callback", json!({})).is_empty());
}

#[test]
fn table_shape_specialization_removes_the_runtime_probe() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let empty = json!([]);
    let kind = empty.is_array();
    Ok(vec![Op::log(kind.to_string())])
}
"#;
    let source = strict_source(handler, EntryKind::Service);
    luau_parses(&source);
    assert!(source.contains("local kind = true"), "{source}");
    assert!(source.contains(noctalia_rs_luau::ARRAY_MARKER), "{source}");
    let values = message(&ops(&source, "update", json!({})));
    assert!(
        !values.is_empty() && values.iter().all(|value| value == "true"),
        "{values:?}"
    );
}

#[test]
fn strict_ast_output_parses_nested_scripts() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "render" {
        let id = "row-\"quoted\"".to_owned();
        let tree = ui::row(json!({ "onClick": ui::callback(&id) }), Vec::new());
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
"#;
    let source = strict_source(handler, EntryKind::Widget);
    luau_parses(&source);
    assert!(!source.contains("onHostCallback"), "{source}");
    assert!(!source.contains("__host_callback"), "{source}");
}
