use crate::{EntryKind, StrictOptions, compile};

use super::{options, source, strict_options};

fn compile_strict(handler: &str) -> Result<String, String> {
    compile(
        &source(handler),
        &strict_options(
            "handler",
            EntryKind::Widget,
            StrictOptions {
                config: "local CONFIG = {}\n",
                translations: "local LANGUAGES = {}\nlocal TRANSLATIONS = {}\n",
                assets: &[],
            },
        ),
    )
    .map(|compiled| compiled.source)
    .map_err(|error| error.render("widget.rs"))
}

const RENDER_WITH_ACTIONS: &str = r#"
use noctalia_rs::{Action, CallbackId, Props};

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "render" {
        let name = ctx.payload_str("action").unwrap_or_default();
        let id = format!("row-{name}");
        let props = Props::new()
            .text("text", "hello")
            .int("fontSize", 12)
            .num("progress", 0.5)
            .bool("focus", true)
            .action("onClick", Action::dynamic(&name))
            .callback("onHover", CallbackId::new(&id));
        let tree = ui::row(props, Vec::new());
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

#[test]
fn strict_typed_props_call_the_api57_node_constructor() {
    let output = compile_strict(RENDER_WITH_ACTIONS).expect("compile");

    assert!(
        output.contains("ui.row("),
        "strict emit did not call the API 57 constructor:\n{output}"
    );
    assert!(output.contains("text = \"hello\""), "{output}");
    assert!(output.contains("fontSize = 12"), "{output}");
    assert!(output.contains("progress = 0.5"), "{output}");
    assert!(output.contains("focus = true"), "{output}");
    assert!(!output.contains("kind = \"row\""), "{output}");
    assert!(!output.contains("__noctalia_rs"), "{output}");
}

#[test]
fn strict_typed_props_keep_assignment_order_and_binding_identity() {
    let handler = r#"
use noctalia_rs::Props;

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "render" {
        let props = Props::new()
            .text("first", format!("{}-1", ctx.event))
            .text("second", format!("{}-2", ctx.event));
        let tree = ui::column(props.clone(), vec![ui::label(props)]);
        return Ok(vec![Op::PanelRender { tree }]);
    }
    Ok(Vec::new())
}
"#;
    let output = compile_strict(handler).expect("compile");

    let first = output.find("first = ").expect("first prop");
    let second = output.find("second = ").expect("second prop");
    assert!(first < second, "{output}");
    assert!(
        output.contains("pairs("),
        "clone must copy the table:\n{output}"
    );
    assert!(output.contains("ui.column("), "{output}");
    assert!(output.contains("ui.label("), "{output}");
}

#[test]
fn strict_typed_handlers_use_action_and_callback_abi() {
    let output = compile_strict(RENDER_WITH_ACTIONS).expect("compile");

    assert!(
        output.contains("__strict_action(__captured, ...)"),
        "{output}"
    );
    assert!(
        output.contains("__strict_callback(__captured, ...)"),
        "{output}"
    );
    assert!(!output.contains("__call ="), "{output}");
    assert!(!output.contains("__host_callback ="), "{output}");
}

#[test]
fn strict_rejects_a_generic_json_prop_map_with_a_location() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let tree = ui::label(json!({ "text": "hi" }));
    Ok(vec![Op::BarRender { tree }])
}
"#;
    let error = compile_strict(handler).expect_err("json prop map must fail");

    assert!(
        error.contains("strict UI props must come from Props::new()"),
        "{error}"
    );
    assert!(
        error.starts_with("widget.rs:6:"),
        "the error must carry file, line and column: {error}"
    );
}

#[test]
fn strict_rejects_an_untyped_json_prop_value() {
    let handler = r#"
use noctalia_rs::Props;

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let tree = ui::label(Props::new().json("extra", json!({ "a": 1 })));
    Ok(vec![Op::BarRender { tree }])
}
"#;
    let error = compile_strict(handler).expect_err("json prop must fail");

    assert!(
        error.contains("json values in props are not supported in strict mode"),
        "{error}"
    );
    assert!(error.starts_with("widget.rs:8:"), "{error}");
}

#[test]
fn strict_rejects_a_dynamic_node_kind() {
    let handler = r#"
use noctalia_rs::Props;

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let kind = ctx.payload_str("kind").unwrap_or_default();
    let tree = ui::node(&kind, Props::new(), Vec::new());
    Ok(vec![Op::BarRender { tree }])
}
"#;
    let error = compile_strict(handler).expect_err("dynamic kind must fail");

    assert!(
        error.contains("strict UI node kinds must be a literal"),
        "{error}"
    );
    assert!(error.starts_with("widget.rs:9:"), "{error}");
}

#[test]
fn strict_rejects_children_on_a_leaf_node() {
    let handler = r#"
use noctalia_rs::Props;

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let tree = ui::node("label", Props::new(), vec![ui::label(Props::new())]);
    Ok(vec![Op::BarRender { tree }])
}
"#;
    let error = compile_strict(handler).expect_err("leaf children must fail");

    assert!(
        error.contains("strict UI node `label` cannot take children"),
        "{error}"
    );
    assert!(error.starts_with("widget.rs:8:"), "{error}");
}

#[test]
fn auto_mode_keeps_generic_json_prop_maps() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let tree = ui::label(json!({ "text": "hi" }));
    Ok(vec![Op::BarRender { tree }])
}
"#;
    let output =
        compile(&source(handler), &options("handler", EntryKind::Widget))
            .expect("compile")
            .source;

    assert!(
        output.contains("kind = \"label\", props = { text = \"hi\" }"),
        "{output}"
    );
}

#[test]
fn auto_mode_evaluates_the_node_kind_before_its_props() {
    let handler = r#"
use noctalia_rs::Props;

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let tree = ui::node(
        ctx.payload_str("kind").unwrap_or_default(),
        Props::new().text("text", ctx.payload_str("text").unwrap_or_default()),
        Vec::new(),
    );
    Ok(vec![Op::BarRender { tree }])
}
"#;
    let output =
        compile(&source(handler), &options("handler", EntryKind::Widget))
            .expect("compile")
            .source;

    let kind = output.find("payload[\"kind\"]").expect("kind read");
    let props = output.find("payload[\"text\"]").expect("prop read");
    assert!(kind < props, "{output}");
}

#[test]
fn strict_rejects_null_typed_handler_names() {
    let handler = r#"
use noctalia_rs::{Action, CallbackId, Props};

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "action" {
        return Ok(vec![Op::log("action")]);
    }
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    let tree = ui::label(
        Props::new()
            .action("onClick", Action::dynamic(json!(null)))
            .callback("onHover", CallbackId::new(json!(null))),
    );
    Ok(vec![Op::BarRender { tree }])
}
"#;
    let error =
        compile_strict(handler).expect_err("null handler name must fail");

    assert!(
        error.contains("json null cannot be stored by an action handler name"),
        "{error}"
    );
}

#[test]
fn strict_rejects_a_null_typed_callback_id() {
    let handler = r#"
use noctalia_rs::{CallbackId, Props};

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    let tree = ui::label(
        Props::new().callback("onHover", CallbackId::new(json!(null))),
    );
    Ok(vec![Op::BarRender { tree }])
}
"#;
    let error =
        compile_strict(handler).expect_err("null callback id must fail");

    assert!(
        error.contains("json null cannot be stored by a callback handler id"),
        "{error}"
    );
}

#[test]
fn strict_rejects_an_unknown_node_kind() {
    let handler = r#"
use noctalia_rs::Props;

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let tree = ui::node("mystery", Props::new(), Vec::new());
    Ok(vec![Op::BarRender { tree }])
}
"#;
    let error = compile_strict(handler).expect_err("unknown kind must fail");

    assert!(
        error.contains("unknown strict UI node kind `mystery`"),
        "{error}"
    );
    assert!(error.starts_with("widget.rs:8:"), "{error}");
}

#[test]
fn strict_brackets_a_keyword_prop_key() {
    let handler = r#"
use noctalia_rs::Props;

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let tree = ui::label(Props::new().text("end", "x"));
    Ok(vec![Op::BarRender { tree }])
}
"#;
    let output = compile_strict(handler).expect("compile");

    assert!(output.contains("[\"end\"] = \"x\""), "{output}");
}
