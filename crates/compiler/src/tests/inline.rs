use super::*;

#[test]
fn rejects_a_missing_handler() {
    let error = compile(
        &source("fn other(_ctx: &Context) -> Result<Vec<Op>> { Ok(vec![]) }"),
        &options("widget", EntryKind::Widget),
    )
    .expect_err("missing");
    assert!(error.message.contains("no function named `widget`"));
}

#[test]
fn rejects_bad_signatures_and_duplicates() {
    for handler in [
        "fn handler() -> Result<Vec<Op>> { Ok(vec![]) }",
        "fn handler(a: &Context, b: &Context) -> Result<Vec<Op>> { let _ = (a, b); Ok(vec![]) }",
        "fn handler(ctx: &str) -> Result<Vec<Op>> { let _ = ctx; Ok(vec![]) }",
    ] {
        assert!(
            compile(&source(handler), &options("handler", EntryKind::Widget))
                .is_err(),
            "should reject: {handler}"
        );
    }
    let duplicate = "mod a { fn handler(_ctx: &Context) -> Result<Vec<Op>> { Ok(vec![]) } }\nmod b { fn handler(_ctx: &Context) -> Result<Vec<Op>> { Ok(vec![]) } }";
    let error =
        compile(&source(duplicate), &options("handler", EntryKind::Widget))
            .expect_err("duplicate");
    assert!(error.message.contains("more than once"));
}

#[test]
fn rejects_local_functions_that_shadow_op_or_ui_names() {
    for name in ["label", "state", "log", "column", "OpenSettings"] {
        let handler = format!(
            "fn {name}() {{}}\nfn handler(ctx: &Context) -> Result<Vec<Op>> {{ let _ = {name}(); Ok(vec![]) }}"
        );
        assert!(
            compile(&source(&handler), &options("handler", EntryKind::Widget))
                .is_err(),
            "should reject shadowing `{name}`"
        );
    }
}

#[test]
fn inlines_a_module_handler() {
    let main = r#"
mod widget;
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    widget::handle(ctx)
}
"#;
    let widget = r#"
use noctalia_rs::{Context, Op, Result};

pub fn handle(ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    ops.push(Op::BarText { text: ctx.config_str("message").unwrap_or_default().into_owned() });
    Ok(ops)
}
"#;
    let modules = [Module {
        path: "widget",
        source: widget,
    }];
    let compiled = compile(
        &source(main),
        &module_options("widget", EntryKind::Widget, &modules),
    )
    .expect("compiles");
    assert!(compiled.source.contains("barWidget.setText("));
    assert!(compiled.source.contains("noctalia.getConfig(\"message\")"));
    assert!(compiled.source.contains("== nil then"));
    assert!(!compiled.source.contains("widget::handle"));
    assert!(compiled.source.contains("do\n"));
}

#[test]
fn inlines_a_mut_ops_helper() {
    let handler = r#"
fn ensure(ctx: &Context, ops: &mut Vec<Op>) {
    if ctx.event == "render" {
        ops.push(Op::BarText { text: "top".to_owned() });
    }
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    ensure(ctx, &mut ops);
    ops.push(Op::log("done"));
    Ok(ops)
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("do\n"));
    assert!(compiled.source.contains("barWidget.setText(\"top\")"));
    assert!(compiled.source.contains("noctalia.log(\"done\")"));
}

#[test]
fn inlines_a_value_helper() {
    let handler = r#"
fn label(ctx: &Context) -> String {
    match ctx.event.as_str() {
        "render" => format!("shown {}", ctx.now_ms),
        _ => "hidden".to_owned(),
    }
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::BarText { text: label(ctx) }])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("local __t1"));
    assert!(
        compiled
            .source
            .contains("__t1 = \"shown \" .. noctalia.nowMs()")
    );
    assert!(compiled.source.contains("barWidget.setText(__t1)"));
}

#[test]
fn resolves_crate_self_and_super_paths() {
    let main = r#"
mod common;
mod widget;
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::log(common::label(ctx))])
}
"#;
    let common = r#"
use noctalia_rs::Context;

pub fn label(ctx: &Context) -> String {
    format!("{}", util::value(ctx))
}

pub fn suffix(_ctx: &Context) -> String {
    "!".to_owned()
}
"#;
    let util = r#"
use noctalia_rs::Context;

pub fn value(ctx: &Context) -> String {
    super::suffix(ctx)
}
"#;
    let modules = [
        Module {
            path: "common",
            source: common,
        },
        Module {
            path: "common::util",
            source: util,
        },
    ];
    let compiled = compile(
        &source(main),
        &module_options("widget", EntryKind::Widget, &modules),
    )
    .expect("compiles");
    assert!(compiled.source.contains("noctalia.log("));
    assert!(compiled.source.contains("\"!\""));
}

#[test]
fn resolves_use_aliases_and_crate_paths() {
    let main = r#"
mod panel;
use crate::panel::handle as panel_handler;
"#;
    let panel = r#"
use noctalia_rs::{Context, Op, Result};

pub fn handle(ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::log(ctx.event.as_str())])
}
"#;
    let modules = [Module {
        path: "panel",
        source: panel,
    }];
    let compiled = compile(
        &source(main),
        &module_options("panel_handler", EntryKind::Panel, &modules),
    )
    .expect("compiles");
    assert!(compiled.source.contains("noctalia.log(\"render\")"));
}

#[test]
fn resolves_constants_across_modules() {
    let main = r#"
mod common;
use crate::common::STATE;

fn widget(_ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::state(STATE, json!("x"))])
}
"#;
    let common = r#"
pub const STATE: &str = "example.state";
"#;
    let modules = [Module {
        path: "common",
        source: common,
    }];
    let compiled = compile(
        &source(main),
        &module_options("widget", EntryKind::Widget, &modules),
    )
    .expect("compiles");
    assert!(compiled.source.contains("state.set(\"example.state\""));
}

#[test]
fn rejects_recursive_and_cyclic_helpers() {
    let handler = r#"
fn first(ctx: &Context, ops: &mut Vec<Op>) {
    second(ctx, ops);
}
fn second(ctx: &Context, ops: &mut Vec<Op>) {
    first(ctx, ops);
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    first(ctx, &mut ops);
    Ok(ops)
}
"#;
    let error =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect_err("cycle");
    assert!(
        error.message.contains("recursive helper"),
        "{}",
        error.message
    );
}

#[test]
fn supports_early_return_and_question_mark_in_helpers() {
    for helper in [
        "fn helper(ctx: &Context, ops: &mut Vec<Op>) { if ctx.now_ms > 0 { return; } ops.push(Op::log(\"x\")); }",
        "fn helper(ctx: &Context, ops: &mut Vec<Op>) { let value = ctx.config_str(\"message\")?; ops.push(Op::log(value)); }",
    ] {
        let handler = format!(
            "{helper}\nfn widget(ctx: &Context) -> Result<Vec<Op>> {{\n    let mut ops = Vec::new();\n    if ctx.event == \"render\" {{ helper(ctx, &mut ops); }}\n    Ok(ops)\n}}\n"
        );
        let compiled =
            compile(&source(&handler), &options("widget", EntryKind::Widget))
                .expect("compiles");
        assert!(compiled.source.contains("(function()"));
        assert!(compiled.source.contains("end)()"));
    }
}

#[test]
fn compiles_closures_in_helpers() {
    let helper = "fn helper(ctx: &Context, ops: &mut Vec<Op>) { let find = |a: i64| a; let _ = find(1); }";
    let handler = format!(
        "{helper}\nfn widget(ctx: &Context) -> Result<Vec<Op>> {{\n    let mut ops = Vec::new();\n    if ctx.event == \"render\" {{ helper(ctx, &mut ops); ops.push(Op::log(\"x\")); }}\n    Ok(ops)\n}}\n"
    );
    let compiled =
        compile(&source(&handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("function(a)"));
}

#[test]
fn copies_back_mut_string_borrows() {
    let handler = r#"
fn helper(word: &mut String) {
    word.push('x');
    word.push_str("y");
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let mut word = String::new();
    helper(&mut word);
    Ok(vec![Op::log(word)])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains(" .. (\"x\")"));
    assert!(compiled.source.contains(" .. (\"y\")"));
    assert!(compiled.source.contains("noctalia.log(word)"));
}

#[test]
fn copies_back_mut_value_reassignment() {
    let handler = r#"
fn apply(value: &mut serde_json::Value, ok: bool) {
    if !ok {
        return;
    }
    *value = json!({ "ready": true });
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let mut value = json!({});
    apply(&mut value, ctx.event == "render");
    Ok(vec![Op::log(value.to_string())])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("= { ready = true }"));
    assert!(compiled.source.contains("do\n"));
}

#[test]
fn copies_back_mut_scalars_after_early_returns() {
    let handler = r#"
fn mark(flag: &mut bool, count: &mut i64, early: bool) {
    if early {
        *flag = true;
        *count = 1;
        return;
    }
    *flag = false;
    *count = 0;
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let mut flag = false;
    let mut count = 0;
    mark(&mut flag, &mut count, ctx.event == "render");
    Ok(vec![Op::log(flag.to_string()), Op::log(count.to_string())])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("= true"));
    assert!(compiled.source.contains("= 1"));
    assert!(compiled.source.matches("flag = ").count() > 2);
    assert!(compiled.source.matches("count = ").count() > 2);
}

#[test]
fn rejects_mut_borrows_that_cannot_be_inlined() {
    let handler = r#"
fn helper(word: &mut &mut String) {
    **word = "x".to_owned();
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let mut word = String::new();
    let mut reference = &mut word;
    helper(&mut reference);
    Ok(vec![Op::log(word)])
}
"#;
    let error =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect_err("bail");
    assert!(error.message.contains("&mut"), "{}", error.message);
}

#[test]
fn rejects_unresolved_module_calls() {
    let handler = r#"
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    missing::handle(ctx)
}
"#;
    let error =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect_err("bail");
    assert!(
        error.message.contains("call to `missing::handle`"),
        "{}",
        error.message
    );
}

#[test]
fn reports_positions_from_the_helper_module() {
    let main = r#"
mod widget;
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    widget::handle(ctx)
}
"#;
    let widget = r#"
use noctalia_rs::{Context, Op, Result};

pub fn handle(_ctx: &Context) -> Result<Vec<Op>> {
    let bad = std::process::id();
    let _ = bad;
    Ok(Vec::new())
}
"#;
    let modules = [Module {
        path: "widget",
        source: widget,
    }];
    let error = compile(
        &source(main),
        &module_options("widget", EntryKind::Widget, &modules),
    )
    .expect_err("bail");
    assert_eq!(
        error.render("main.rs"),
        format!("widget:{}:{}: {}", error.line, error.column, error.message)
    );
    assert!(error.line > 0);
}

#[test]
fn finds_a_handler_in_an_inline_module() {
    let handler = r#"
mod inner {
    pub fn widget(_ctx: &Context) -> Result<Vec<Op>> {
        Ok(vec![Op::log("inner")])
    }
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("noctalia.log(\"inner\")"));
}

#[test]
fn resolves_self_paths_glob_imports_and_super_chains() {
    let main = r#"
mod outer;
use crate::shared::*;
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::log(self::local(ctx)), Op::log(globbed(ctx))])
}
fn local(_ctx: &Context) -> String {
    "local".to_owned()
}
"#;
    let outer = r#"
mod inner;

pub fn label(ctx: &Context) -> String {
    super::super::local(ctx)
}
"#;
    let inner = r#"
use crate::outer::label;
use noctalia_rs::Context;

pub fn deep(ctx: &Context) -> String {
    self::suffix(ctx)
}

fn suffix(_ctx: &Context) -> String {
    label(ctx)
}
"#;
    let shared = r#"
use noctalia_rs::Context;

pub fn globbed(_ctx: &Context) -> String {
    "glob".to_owned()
}
"#;
    let modules = [
        Module {
            path: "outer",
            source: outer,
        },
        Module {
            path: "outer::inner",
            source: inner,
        },
        Module {
            path: "shared",
            source: shared,
        },
    ];
    let compiled = compile(
        &source(main),
        &module_options("widget", EntryKind::Widget, &modules),
    )
    .expect("compiles");
    assert!(compiled.source.contains("\"local\""));
    assert!(compiled.source.contains("\"glob\""));
}

#[test]
fn resolves_constants_through_self_and_super() {
    let main = r#"
mod common;
fn widget(_ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::state(common::STATE, json!("x"))])
}
"#;
    let common = r#"
pub const STATE: &str = "example.state";
pub mod nested {
    pub const KEY: &str = "nested.key";
}
"#;
    let modules = [Module {
        path: "common",
        source: common,
    }];
    let compiled = compile(
        &source(main),
        &module_options("widget", EntryKind::Widget, &modules),
    )
    .expect("compiles");
    assert!(compiled.source.contains("state.set(\"example.state\""));
}

#[test]
fn inlines_vec_ops_helpers_and_final_returns() {
    let handler = r#"
fn build(ctx: &Context) -> Vec<Op> {
    let mut ops = Vec::new();
    ops.push(Op::log(ctx.event.as_str()));
    ops
}
fn offset(ctx: &Context) -> i64 {
    return ctx.now_ms;
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let now = offset(ctx);
    Ok(build(ctx))
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("noctalia.log(\"render\")"));
}

#[test]
fn rejects_inline_signature_mismatches() {
    for handler in [
        r#"
fn helper(ctx: &Context) {}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    helper(ctx, ctx);
    Ok(Vec::new())
}
"#,
        r#"
fn helper(other: &Context) { let _ = other; }
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    helper(missing);
    Ok(Vec::new())
}
"#,
        r#"
fn helper(ops: &mut Vec<Op>) {}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    helper(&mut Vec::new());
    Ok(Vec::new())
}
"#,
        r#"
fn helper(ctx: &Context) -> Result<String> { Ok(String::new()) }
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    helper(ctx)
}
"#,
        r#"
fn helper(ctx: &Context) -> Vec<Op> { Vec::new() }
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    helper(ctx)
}
"#,
        r#"
fn helper(ctx: &Context) -> Result<Vec<Op>> { Ok(Vec::new()) }
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let value = helper(ctx);
    Ok(value)
}
"#,
        r#"
        r#"
fn helper(ctx: &Context) -> String { let _ = ctx; }
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let value = helper(ctx);
    Ok(vec![Op::log(value)])
}
"#,
    ] {
        assert!(
            compile(&source(handler), &options("widget", EntryKind::Widget))
                .is_err(),
            "should bail: {handler}"
        );
    }
}

#[test]
fn rejects_inline_depth_and_unsupported_ops() {
    let mut handler = String::new();
    for index in 0..10 {
        handler.push_str(&format!(
            "fn helper{index}(ctx: &Context, ops: &mut Vec<Op>) {{\n    helper{}(ctx, ops);\n}}\n",
            index + 1
        ));
    }
    handler.push_str("fn helper10(_ctx: &Context, _ops: &mut Vec<Op>) {}\n");
    handler.push_str(
        "fn widget(ctx: &Context) -> Result<Vec<Op>> {\n    let mut ops = Vec::new();\n    helper0(ctx, &mut ops);\n    Ok(ops)\n}\n",
    );
    let error =
        compile(&source(&handler), &options("widget", EntryKind::Widget))
            .expect_err("depth");
    assert!(error.message.contains("depth limit"), "{}", error.message);

    let handler = r#"
fn widget(_ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::query("id", "noctalia", &["nowMs"], vec![], "out.json")])
}
"#;
    let error =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect_err("query");
    assert!(error.message.contains("response path"), "{}", error.message);
}

#[test]
fn rejects_parse_errors_and_renders_them() {
    let error = compile(
        "fn widget(ctx: &Context) -> Result<Vec<Op>> { Ok(vec![Op::Nope]) }",
        &options("widget", EntryKind::Widget),
    )
    .expect_err("op");
    assert!(error.line > 0);
    assert!(error.to_string().contains("line "));
    assert!(error.render("main.rs").contains("main.rs:"));
    let plain = crate::Error::plain("plain");
    assert_eq!(plain.render("main.rs"), "main.rs: plain");
    let parse = compile("fn {", &options("widget", EntryKind::Widget))
        .expect_err("parse");
    assert!(parse.render("main.rs").contains("main:"));
}

#[test]
fn covers_local_statics_and_guard_helpers() {
    let handler = r#"
use std::sync::{Once, OnceLock};
fn guard(ctx: &Context, ops: &mut Vec<Op>) {
    static LOCAL_ONCE: Once = Once::new();
    static LOCAL_VALUE: OnceLock<String> = OnceLock::new();
    LOCAL_ONCE.call_once(|| ops.push(Op::log("local")));
    let value = LOCAL_VALUE.get_or_init(|| ctx.tr("key", &[]));
    ops.push(Op::log(value));
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    guard(ctx, &mut ops);
    Ok(ops)
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("__local_"));
}

#[test]
fn scans_helper_expressions_and_inlines_a_manual_default() {
    let handler = r#"
struct State { count: i64 }
impl State {
    fn default() -> State { State { count: 1 } }
    fn scan(&self, ops: &mut Vec<Op>) {
        let value = -1;
        let wrapped = (value);
        let reference = &wrapped;
        let mut total = 0;
        total += *reference;
        let cast = total as f64;
        let array = [1, 2];
        let tuple = (1, 2);
        let nested = State { count: 2 };
        let _ = array[0];
        let _ = nested.count;
        let _ = tuple.0;
        let _ = wrapped.abs();
        let _ = format!("{}", cast);
        ops.push(Op::log("scanned"));
    }
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let state = State::default();
    let mut ops = Vec::new();
    state.scan(&mut ops);
    let _ = ctx.now_ms;
    Ok(ops)
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("count = 1"));
    assert!(compiled.source.contains("noctalia.log(\"scanned\")"));
}

#[test]
fn rejects_a_void_helper_in_value_position() {
    let handler = r#"
fn helper() {}
fn widget(_ctx: &Context) -> Result<Vec<Op>> {
    let value = helper();
    let _ = value;
    Ok(vec![])
}
"#;
    let error =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect_err("void helper");
    assert!(
        error.message.contains("returns no value"),
        "{}",
        error.message
    );
}

#[test]
fn scans_loop_and_let_bindings_in_helpers() {
    let handler = r#"
fn helper(ctx: &Context, ops: &mut Vec<Op>) {
    let mut total = 0;
    loop {
        total += 1;
        break;
    }
    if let Some(value) = ctx.config_str("message") {
        total += value.len() as i64;
    }
    total = total + 1;
    ops.push(Op::log(format!("{}", total)));
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    helper(ctx, &mut ops);
    Ok(ops)
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("while true do"));
    assert!(compiled.source.contains("break"));
}

#[test]
fn unit_helpers_compile_inside_closures() {
    let handler = r#"
fn refresh(_ctx: &Context, ops: &mut Vec<Op>) {
    ops.push(Op::log("tick"));
}
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    static INIT: Once = Once::new();
    INIT.call_once(|| refresh(ctx, &mut ops));
    Ok(ops)
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("noctalia.log(\"tick\")"),
        "{}",
        compiled.source
    );
}

#[test]
fn compiles_glob_imported_constants_and_associated_values() {
    let main = r#"
mod state;
use state::*;
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = STATE_KEY;
    let preset = Preset::EMPTY;
    Ok(vec![Op::log(format!("{value} {preset}"))])
}
"#;
    let state = r#"
pub const STATE_KEY: &str = "me.state";
pub struct Preset { pub name: String }
impl Preset {
    pub const EMPTY: Preset = Preset { name: String::new() };
}
"#;
    let modules = [Module {
        path: "state",
        source: state,
    }];
    let compiled = compile(
        &source(main),
        &module_options("handler", EntryKind::Service, &modules),
    )
    .expect("compiles");
    assert!(compiled.source.contains("me.state"), "{}", compiled.source);
}

#[test]
fn copies_back_after_one_line_iife_returns() {
    let handler = r#"
fn mark(flag: &mut bool, count: &mut i64, text: &str) {
    let at = text.find("x");
    *flag = at.is_some();
    *count = 1;
    if !*flag {
        return;
    }
    *flag = false;
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let mut flag = false;
    let mut count = 0;
    mark(&mut flag, &mut count, ctx.event.as_str());
    Ok(vec![Op::log(flag.to_string()), Op::log(count.to_string())])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(
        compiled.source.matches("flag = ").count() > 2,
        "{}",
        compiled.source
    );
    assert!(compiled.source.matches("count = ").count() > 2);
}

#[test]
fn rejects_index_operands_that_are_mutated_in_the_same_call() {
    let handler = r#"
fn bump(value: &mut i64, index: &mut i64) {
    *value = 5;
    *index = 1;
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let mut nums: Vec<i64> = vec![0, 0, 0];
    let mut index = 0;
    bump(&mut nums[index], &mut index);
    Ok(vec![Op::log(nums[0].to_string())])
}
"#;
    let error =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect_err("bail");
    assert!(
        error.message.contains("cannot preserve `&mut` aliasing"),
        "{}",
        error.message
    );
}

#[test]
fn keeps_immutable_index_operands() {
    let handler = r#"
fn bump(value: &mut i64, index: i64) {
    *value = 5;
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let mut nums: Vec<i64> = vec![0, 0, 0];
    let index = 0;
    bump(&mut nums[index], index);
    Ok(vec![Op::log(nums[0].to_string())])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("nums[(index) + 1] = value"));
}

#[test]
fn rejects_unsupported_string_mutators() {
    let handler = r#"
fn replace(text: &mut String) {
    text.insert(1, 'x');
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let mut text = String::new();
    replace(&mut text);
    Ok(vec![Op::log(text)])
}
"#;
    let error =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect_err("bail");
    assert!(
        error.message.contains("String::insert"),
        "{}",
        error.message
    );
}

#[test]
fn keeps_nested_helper_copy_backs_in_their_own_frame() {
    let handler = r#"
fn inner(n: &mut i64) -> i64 {
    *n = 9;
    *n
}
fn outer(n: &mut i64, p: &mut i64) -> i64 {
    *n = inner(p);
    *n
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let mut total = 0;
    let mut source = 1;
    let value = outer(&mut total, &mut source);
    Ok(vec![Op::log(value.to_string()), Op::log(total.to_string())])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("(function()"));
    assert_eq!(compiled.source.matches("total = n").count(), 2);
}

#[test]
fn rejects_shadowed_mut_parameters() {
    let handler = r#"
fn helper(n: &mut i64) -> i64 {
    *n = 1;
    let n = 2;
    n + 3
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let mut n = 0;
    let value = helper(&mut n);
    Ok(vec![Op::log(value.to_string()), Op::log(n.to_string())])
}
"#;
    let error =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect_err("bail");
    assert!(error.message.contains("shadowed"), "{}", error.message);
}

#[test]
fn rejects_aliased_mut_string_reborrows() {
    let handler = r#"
fn helper(text: &mut String) {
    let alias = &mut *text;
    alias.push_str("x");
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let mut text = String::new();
    helper(&mut text);
    Ok(vec![Op::log(text)])
}
"#;
    let error =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect_err("bail");
    assert!(
        error.message.contains("mutable reborrows"),
        "{}",
        error.message
    );
}

#[test]
fn keeps_non_shadowing_mut_parameters() {
    let handler = r#"
fn helper(n: &mut i64) -> i64 {
    *n = 1;
    let extra = 2;
    *n + extra + 2
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let mut total = 0;
    let value = helper(&mut total);
    Ok(vec![Op::log(value.to_string()), Op::log(total.to_string())])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("((n + extra) + 2)"));
}
