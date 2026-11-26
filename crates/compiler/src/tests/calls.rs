use super::*;

#[test]
fn compiles_ui_trees_to_table_literals() {
    let handler = r#"
fn panel(ctx: &Context) -> Result<Vec<Op>> {
    let label = ctx.state_value::<String>("label").unwrap_or_default();
    let tree = ui::column(
        json!({ "id": "root", "gap": 4 }),
        vec![
            ui::label(json!({ "id": "value", "text": label })),
            ui::button(json!({ "id": "save", "label": "Save" })),
        ],
    );
    Ok(vec![Op::PanelRender { tree }])
}
"#;
    let compiled =
        compile(&source(handler), &options("panel", EntryKind::Panel))
            .expect("compiles");

    assert!(
        compiled
            .source
            .contains("local plugin_state = state.get(STATE_KEY) or {}")
    );
    assert!(compiled.source.contains("plugin_state[\"label\"]"));
    assert!(compiled.source.contains("== nil then __t"));
    assert!(compiled.source.contains(
        "{ kind = \"column\", props = { id = \"root\", gap = 4 }, children = {"
    ));
    assert!(compiled.source.contains("kind = \"label\""));
    assert!(compiled.source.contains("id = \"value\""));
    assert!(compiled.source.contains("text = label"));
    assert!(compiled.source.contains("children = {}"));
    assert!(compiled.source.contains("panel.render(tree)"));
    assert!(compiled.source.contains("panel.setWantsSecondTicks(true)"));
    assert!(compiled.source.contains("--!nonstrict"));
}

#[test]
fn lazy_fallback_closures_stay_inside_the_nil_branch() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let value = ctx.state_value::<String>("value").map_or_else(
        || "fallback".to_owned(),
        |text| text,
    );
    let _ = ctx.state_value::<String>("other").ok_or_else(|| "missing");
    Ok(vec![Op::log(value)])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled
            .source
            .contains("local __t1 = plugin_state[\"value\"]")
    );
    assert!(compiled.source.contains("if __t1 == nil then"));
    assert!(compiled.source.contains("if __t3 == nil then"));
    assert!(
        !compiled
            .source
            .contains("local __t4 = error(tostring(\"missing\"))")
    );
}

#[test]
fn compiles_ui_call_markers() {
    let handler = r#"
fn widget(_ctx: &Context) -> Result<Vec<Op>> {
    let props = json!({ "onClick": ui::call("onRefresh"), "onRightClick": ui::callback("row") });
    Ok(vec![Op::BarRender { tree: ui::button(props) }])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(
        compiled.source.contains("{ __call = \"onRefresh\" }"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("{ __host_callback = \"row\" }"),
        "{}",
        compiled.source
    );
}

#[test]
fn compiles_function_references_as_callbacks() {
    let handler = r#"
fn text(value: Value) -> String {
    value.as_str().unwrap_or_default().to_owned()
}
fn snapshot() -> Value {
    json!({})
}
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let named = ctx.config.get("name").map(text).unwrap_or_default();
    let fallback = ctx.config.get("grid").cloned().unwrap_or_else(snapshot);
    let empty = ctx.config.get("list").and_then(Value::as_array).is_some_and(Vec::is_empty);
    let owned = ctx.config.get("list").and_then(Value::as_array).map_or(Vec::new(), |list| list.clone());
    Ok(vec![Op::log(format!("{named} {empty} {fallback} {owned}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let generated = &compiled.source;
    assert!(generated.contains("local value = __t"), "{generated}");
    assert!(generated.contains("#__value == 0"), "{generated}");
    assert!(generated.contains("local list = __t"), "{generated}");
}

#[test]
fn accepts_duplicate_globs_of_one_function() {
    let handler = r#"
use crate::a::*;
use crate::b::*;
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let text = shared();
    Ok(vec![Op::log(text)])
}
"#;
    let modules = [
        Module {
            path: "common",
            source: "pub fn shared() -> String { \"ok\".to_owned() }",
        },
        Module {
            path: "a",
            source: "pub use crate::common::*;",
        },
        Module {
            path: "b",
            source: "pub use crate::common::*;",
        },
    ];
    let compiled = compile(
        &source(handler),
        &module_options("handler", EntryKind::Service, &modules),
    )
    .expect("compiles");
    assert!(
        compiled.source.contains("noctalia.log(text)")
            && compiled.source.contains("\"ok\""),
        "{}",
        compiled.source
    );
}

#[test]
fn rejects_duplicate_globs_of_different_functions() {
    let handler = r#"
use crate::a::*;
use crate::b::*;
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let text = shared();
    Ok(vec![Op::log(text)])
}
"#;
    let modules = [
        Module {
            path: "a",
            source: "pub fn shared() -> String { \"a\".to_owned() }",
        },
        Module {
            path: "b",
            source: "pub fn shared() -> String { \"b\".to_owned() }",
        },
    ];
    let error = compile(
        &source(handler),
        &module_options("handler", EntryKind::Service, &modules),
    )
    .expect_err("ambiguous");
    assert!(
        error.message.contains("no function in the crate matches"),
        "{}",
        error.message
    );
    assert!(error.line > 0 && error.column > 0, "{error:?}");
}
