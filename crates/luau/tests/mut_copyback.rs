use std::path::PathBuf;

use noctalia_rs::{Context, Parts};
use noctalia_rs_compiler::{EntryKind, NativeMode, Options, compile};
use serde_json::{Value, json};

fn context(event: &str) -> Context {
    Context::new(
        Parts {
            entry: "mut_copyback".to_owned(),
            event: event.to_owned(),
            payload: json!({}),
            config: json!({}),
            state: json!({}),
            host: json!({}),
            data_dir: PathBuf::from("/tmp/noctalia-mut-copyback"),
        },
        json!({}),
    )
}

fn compiled(handler: &str) -> String {
    let source = format!(
        "use noctalia_rs::{{Context, Op, Result}};\nuse serde_json::json;\n\n{handler}\n"
    );
    compile(
        &source,
        &Options {
            handler: "handler",
            entry: "mut_copyback",
            state_key: "test.state",
            kind: EntryKind::Widget,
            strings: &[],
            settings: &[],
            module: "main",
            modules: &[],
            native: NativeMode::Deny,
            strict: None,
        },
    )
    .expect("compile")
    .source
}

fn logs(source: &str, event: &str) -> Vec<String> {
    noctalia_rs_luau::dispatch(&context(event), &[("mut_copyback", source)])
        .expect("dispatch")
        .into_iter()
        .filter_map(|op| match serde_json::to_value(op).unwrap() {
            Value::Object(mut item) => match item.remove("message") {
                Some(Value::String(message)) => Some(message),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn native_bump(value: &mut i64, index: i64) {
    *value = index + 5;
}

fn native_numbers() -> String {
    let mut numbers = [0i64, 0, 0];
    let index = 0usize;
    native_bump(&mut numbers[index], 0);
    numbers[0].to_string()
}

fn native_mark(text: &str, early: bool) -> String {
    let found = text.find('x').is_some();
    let flag = early || found;
    let count = if early { 1 } else { 2 };
    format!("{flag}:{count}")
}

fn native_nested() -> Vec<String> {
    vec!["9".to_owned(), "9".to_owned()]
}

#[test]
fn one_line_iife_returns_still_copy_back() {
    let source = compiled(
        r#"
fn mark(flag: &mut bool, count: &mut i64, early: bool, text: &str) {
    let at = text.find("x");
    *flag = at.is_some();
    *count = 1;
    if early {
        *flag = true;
        return;
    }
    *flag = false;
    *count = 2;
}
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let mut flag = false;
    let mut count = 0;
    mark(&mut flag, &mut count, ctx.event == "render", ctx.event.as_str());
    Ok(vec![Op::log(format!("{}:{}", flag, count))])
}
"#,
    );
    let event = "render";
    let expected = native_mark(event, true);
    let actual = logs(&source, event);
    assert!(!actual.is_empty(), "no logs for {event}");
    assert!(
        actual.iter().all(|message| message == &expected),
        "{event}: {actual:?} != {expected}"
    );
}

#[test]
fn immutable_index_operand_matches_native_handler() {
    let source = compiled(
        r#"
fn bump(value: &mut i64, index: i64) {
    *value = index + 5;
}
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let mut numbers: Vec<i64> = vec![0, 0, 0];
    let index = 0;
    bump(&mut numbers[index], index);
    Ok(vec![Op::log(numbers[0].to_string())])
}
"#,
    );
    let expected = native_numbers();
    let actual = logs(&source, "render");
    assert!(!actual.is_empty());
    assert!(actual.iter().all(|message| message == &expected));
}

#[test]
fn string_push_str_matches_native_handler() {
    let source = compiled(
        r#"
fn append(text: &mut String) {
    text.push_str("ab");
}
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let mut text = String::new();
    append(&mut text);
    Ok(vec![Op::log(text)])
}
"#,
    );
    let mut native = String::new();
    native.push_str("ab");
    let actual = logs(&source, "render");
    assert!(!actual.is_empty());
    assert!(actual.iter().all(|message| message == &native));
}

#[test]
fn nested_helper_returns_match_native_handler() {
    let source = compiled(
        r#"
fn inner(n: &mut i64) -> i64 {
    *n = 9;
    *n
}
fn outer(n: &mut i64, p: &mut i64) -> i64 {
    *n = inner(p);
    *n
}
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let mut total = 0;
    let mut source = 1;
    let value = outer(&mut total, &mut source);
    Ok(vec![Op::log(value.to_string()), Op::log(total.to_string())])
}
"#,
    );
    let expected = native_nested();
    let actual = logs(&source, "render");
    assert!(!actual.is_empty());
    assert_eq!(actual.len() % expected.len(), 0);
    for chunk in actual.chunks(expected.len()) {
        assert_eq!(chunk, expected);
    }
}

fn compile_result(handler: &str) -> Result<String, String> {
    let source = format!(
        "use noctalia_rs::{{Context, Op, Result}};\nuse serde_json::json;\n\n{handler}\n"
    );
    compile(
        &source,
        &Options {
            handler: "handler",
            entry: "mut_copyback",
            state_key: "test.state",
            kind: EntryKind::Widget,
            strings: &[],
            settings: &[],
            module: "main",
            modules: &[],
            native: NativeMode::Deny,
            strict: None,
        },
    )
    .map(|result| result.source)
    .map_err(|error| error.message)
}

#[test]
fn shadowed_mut_parameter_is_rejected() {
    let error = compile_result(
        r#"
fn helper(n: &mut i64) -> i64 {
    *n = 1;
    let n = 2;
    n + 3
}
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let mut n = 0;
    let value = helper(&mut n);
    Ok(vec![Op::log(value.to_string()), Op::log(n.to_string())])
}
"#,
    )
    .expect_err("shadowing must fail");
    assert!(error.contains("shadowed"), "{error}");
}

#[test]
fn non_shadowing_mut_parameter_matches_native_handler() {
    let source = compiled(
        r#"
fn helper(n: &mut i64) -> i64 {
    *n = 1;
    let extra = 2;
    *n + extra + 2
}
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let mut n = 0;
    let value = helper(&mut n);
    Ok(vec![Op::log(value.to_string()), Op::log(n.to_string())])
}
"#,
    );
    let expected = ["5", "1"];
    let actual = logs(&source, "render");
    assert!(!actual.is_empty());
    for chunk in actual.chunks(expected.len()) {
        assert_eq!(chunk, expected);
    }
}

#[test]
fn mut_string_reborrow_is_rejected() {
    let error = compile_result(
        r#"
fn helper(text: &mut String) {
    let alias = &mut *text;
    alias.push_str("x");
}
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let mut text = String::new();
    helper(&mut text);
    Ok(vec![Op::log(text)])
}
"#,
    )
    .expect_err("aliased reborrow must fail");
    assert!(error.contains("mutable reborrows"), "{error}");
}
