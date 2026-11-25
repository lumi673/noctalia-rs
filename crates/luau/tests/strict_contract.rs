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
