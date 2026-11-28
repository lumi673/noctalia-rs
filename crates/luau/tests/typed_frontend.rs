//! Runtime proof for the typed HIR frontend.

use std::path::PathBuf;

use noctalia_rs::compat::Parts;
use noctalia_rs::{Context, Op};
use noctalia_rs_compiler::frontend::compile_typed;
use noctalia_rs_compiler::{EntryKind, NativeMode, StrictOptions};
use serde_json::{Value, json};

fn context(entry: &str, event: &str, payload: Value) -> Context {
    Context::new(
        Parts {
            entry: entry.to_owned(),
            event: event.to_owned(),
            payload,
            config: json!({}),
            state: json!({}),
            host: json!({}),
            data_dir: PathBuf::from("/tmp/noctalia-typed-frontend"),
        },
        json!({}),
    )
}

fn rust_ops(ops: Vec<Op>) -> Vec<Value> {
    ops.iter()
        .filter(|op| !matches!(op, Op::HostCall { .. }))
        .map(|op| serde_json::to_value(op).expect("serialize"))
        .collect()
}

fn compile_handler(handler: &str, strict: bool) -> String {
    let source = format!(
        "use noctalia_rs::{{Context, Op, Result}};\nuse serde_json::json;\n\n{handler}\n"
    );
    let assets: Vec<(&str, u32)> = Vec::new();
    let strict = strict.then_some(StrictOptions {
        config: "local CONFIG = {}\n",
        translations: "local LANGUAGES = {}\nlocal TRANSLATIONS = {}\n",
        assets: &assets,
    });
    compile_typed(
        &source,
        &noctalia_rs_compiler::Options {
            handler: "handler",
            entry: "typed",
            state_key: "typed.state",
            kind: EntryKind::Widget,
            strings: &[],
            settings: &[],
            module: "main",
            modules: &[],
            native: NativeMode::Deny,
            strict,
        },
    )
    .expect("compile")
    .source
}

#[test]
fn expanded_macros_and_resolved_traits_run_in_the_runtime() {
    let script = compile_handler(
        r#"
trait Label { fn label(&self) -> &'static str; }
struct A { name: &'static str }
impl Label for A { fn label(&self) -> &'static str { "first" } }
macro_rules! emit { ($value:expr) => { Op::log($value) }; }
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let item = A { name: "unused" };
    Ok(vec![emit!(item.label())])
}
"#,
        false,
    );
    let ctx = context("typed", "render", json!({ "source": "update" }));
    let actual = noctalia_rs_luau::dispatch(&ctx, &[("typed", &script)])
        .expect("dispatch");
    assert_eq!(
        rust_ops(actual),
        rust_ops(vec![
            Op::log("first".to_owned()),
            Op::log("first".to_owned())
        ])
    );
}

#[test]
fn strict_typed_script_parses_and_drives_the_api() {
    let script = compile_handler(
        r#"
const LIMIT: u8 = 200 + 55;
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::log(format!("{}", LIMIT))])
}
"#,
        true,
    );
    let ctx = context("typed", "render", json!({ "source": "update" }));
    let actual = noctalia_rs_luau::dispatch_strict(&ctx, &[("typed", &script)])
        .expect("dispatch strict");
    assert_eq!(
        rust_ops(actual),
        rust_ops(vec![Op::log("255".to_owned()), Op::log("255".to_owned())])
    );
}

#[test]
fn closure_capture_and_shadowed_initializer_run_correctly() {
    let script = compile_handler(
        r#"
fn helper(value: &str) -> &str { value }
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let label = "outer";
    let label = helper(label);
    let show = || label;
    Ok(vec![Op::log(show())])
}
"#,
        false,
    );
    let ctx = context("typed", "render", json!({ "source": "update" }));
    let actual = noctalia_rs_luau::dispatch(&ctx, &[("typed", &script)])
        .expect("dispatch");
    assert_eq!(
        rust_ops(actual),
        rust_ops(vec![
            Op::log("outer".to_owned()),
            Op::log("outer".to_owned())
        ])
    );
}

#[test]
fn trait_method_arguments_run_in_the_runtime() {
    let script = compile_handler(
        r#"
trait Add { fn add(&self, b: i32) -> i32; }
struct A { value: i32 }
impl Add for A { fn add(&self, b: i32) -> i32 { b + self.value } }
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let a = A { value: 1 };
    Ok(vec![Op::log(a.add(2).to_string())])
}
"#,
        false,
    );
    let ctx = context("typed", "render", json!({ "source": "update" }));
    let actual = noctalia_rs_luau::dispatch(&ctx, &[("typed", &script)])
        .expect("dispatch");
    assert_eq!(
        rust_ops(actual),
        rust_ops(vec![Op::log("3".to_owned()), Op::log("3".to_owned())])
    );
}
