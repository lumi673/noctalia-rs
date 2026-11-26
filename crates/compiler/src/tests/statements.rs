use super::*;

#[test]
fn rejects_constructs_outside_the_subset() {
    for handler in [
        "fn handler(ctx: &Context) -> Result<Vec<Op>> { let x = ctx.calendar_events(\"a\"); Ok(vec![]) }",
        "fn handler(ctx: &Context) -> Result<Vec<Op>> { match ctx.event.as_str() { other => { let _ = other; Ok(Vec::new()) } } }",
        "fn handler(ctx: &Context) -> Result<Vec<Op>> { let x = std::process::id(); Ok(vec![]) }",
    ] {
        assert!(
            compile(&source(handler), &options("handler", EntryKind::Widget))
                .is_err(),
            "should reject: {handler}"
        );
    }
}

#[test]
fn accepts_a_tail_return() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "update" {
        return Ok(vec![Op::log("tick")]);
    } else {
        return Ok(Vec::new());
    }
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled
            .source
            .contains("function update() body_update() end")
    );
    assert!(compiled.source.contains("noctalia.log(\"tick\")"));
}

#[test]
fn wraps_early_returns_in_an_iife_inside_nested_loops() {
    let handler = r#"
fn helper(ops: &mut Vec<Op>) {
    for index in 0..3 {
        if index == 1 {
            return;
        }
        ops.push(Op::log(format!("{}", index)));
    }
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    helper(&mut ops);
    Ok(ops)
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("(function()"));
    assert!(compiled.source.contains("end)()"));
    assert!(compiled.source.contains("for index = 0, (3) - 1 do"));
}

#[test]
fn compiles_if_let_matches_and_tuple_bindings() {
    let handler = r#"
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let (first, second) = ("a", "b");
    if let Some(value) = ctx.config_str("message") {
        if matches!(value.as_str(), Some(_)) {
            return Ok(vec![Op::log(format!("{}{}", first, second))]);
        }
    }
    let Some(fallback) = ctx.config_str("message") else {
        return Ok(vec![Op::log("fallback")]);
    };
    Ok(vec![Op::log(fallback)])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("local first, second ="));
    assert!(compiled.source.contains("local value = __t"));
    assert!(!compiled.source.contains("if __static"));
    assert!(compiled.source.contains("if (((value ~= nil)))"));
    assert!(!compiled.source.contains("if __static"));
}

#[test]
fn bails_on_constructs_without_a_safe_translation() {
    for handler in [
        "fn widget(_ctx: &Context) -> Result<Vec<Op>> { Ok(vec![Op::http_bytes(json!({}), 1)]) }",
        "fn widget(_ctx: &Context) -> Result<Vec<Op>> { other = 1; Ok(vec![]) }",
    ] {
        let error =
            compile(&source(handler), &options("widget", EntryKind::Widget))
                .expect_err("bail");
        assert!(error.line > 0 && error.column > 0);
        assert!(!error.message.is_empty());
    }
}

#[test]
fn covers_match_and_matches_patterns() {
    let handler = r#"
enum Mode { Fast, Slow }
fn classify(value: Option<&Value>, mode: &str) -> String {
    match value {
        Some(Value::Number(number)) => number.to_string(),
        Some(Value::String(text)) => text.trim().to_owned(),
        Some(Value::Bool(flag)) => flag.to_string(),
        Some(Value::Object(map)) => map.len().to_string(),
        Some(Value::Array(items)) => items.len().to_string(),
        None => "none".to_owned(),
        Some(Value::Null) => "null".to_owned(),
    }
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let value = ctx.config_str("message").and_then(Value::as_str);
    let label = classify(value, "Fast");
    let known = matches!(label.as_str(), "Fast" | "Slow");
    let object = matches!(label.as_str(), "" );
    let mode = Mode::Fast;
    let other = match mode { Mode::Fast => "f", Mode::Slow => "s" };
    Ok(vec![Op::log(format!("{}{}{}{}", label, known, object, other))])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("type("));
}

#[test]
fn covers_more_match_variants_and_bails() {
    let handler = r#"
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let value = ctx.config_str("message").and_then(Value::as_str);
    let label = match value {
        Some("yes") => "on",
        Some(_) => "other",
        None => "off",
    };
    let tuple = match value { Some(text) => (text, 1), None => ("none", 0) };
    let flag = matches!(value, Some(_) | None);
    Ok(vec![Op::log(format!("{}{}{}{}", label, tuple.0, tuple.1, flag))])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("tuple[1]"));
}

#[test]
fn covers_final_error_and_pattern_branches() {
    let handler = r#"
use std::sync::OnceLock;
static READY: OnceLock<bool> = OnceLock::new();
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let ready = READY.is_completed();
    let value = ctx.config_str("message");
    let mapped = value.clone().map_or(1, |text| text.len() as i64);
    let kept = value.clone().filter(|text| matches!(text.as_str(), "" | "x"));
    Ok(vec![Op::log(format!("{}{}{}", ready, mapped, kept.unwrap_or_default()))])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("~= nil"));

    for handler in [
        "fn widget(ctx: &Context) -> Result<Vec<Op>> { let value = ctx.config_str(\"message\").map(|a: i64, b: i64| a + b); let _ = value; Ok(vec![]) }",
        "fn helper(ctx: &Context) -> String { let _ = ctx; } fn widget(ctx: &Context) -> Result<Vec<Op>> { let v = helper(ctx); Ok(vec![Op::log(v)]) }",
    ] {
        let error =
            compile(&source(handler), &options("widget", EntryKind::Widget))
                .expect_err("bail");
        assert!(!error.message.is_empty());
    }
}

#[test]
fn compiles_nested_table_assignment() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let mut state = ctx.state;
    state.inner.value = 1;
    state.rows[0] = json!({ "ok": true });
    Ok(Vec::new())
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("state.inner.value = 1"));
    assert!(compiled.source.contains("state.rows[(0) + 1] ="));
}

#[test]
fn compiles_nested_matches_with_enum_and_value_patterns() {
    let handler = r#"
enum Mode { Fast, Slow }
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mode = Mode::Fast;
    let value = serde_json::from_str::<Value>("{}").ok();
    let fast = matches!(mode, Mode::Fast);
    let text = matches!(value, Some(Value::String(_)));
    let typed = matches!(value, Some(Value::Number(_)) | Some(Value::Bool(_)));
    let object = matches!(value, Some(Value::Object(_)));
    let none = matches!(value, None);
    Ok(vec![Op::log(format!("{}{}{}{}{}", fast, text, typed, object, none))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("type("));
    assert!(compiled.source.contains(" or "));
    assert!(compiled.source.contains(" == nil"));
}

#[test]
fn covers_nested_match_pattern_branches() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = serde_json::from_str::<Value>("{}").ok();
    let literal = matches!(value, Some(Value::String("x")));
    let reference = matches!(value, Some(&Some(_)));
    let paren = matches!(value, Some((_)));
    let path = matches!(value, Some(None));
    let inner = matches!(value, Some(Some(_)));
    let number = matches!(value, Some(Value::Number(_)));
    let array = matches!(value, Some(Value::Array(_)));
    let boolean = matches!(value, Some(Value::Bool(_)));
    let object = matches!(value, Some(Value::Object(_)));
    Ok(vec![Op::log(format!("{}{}{}{}{}{}{}{}{}", literal, reference, paren, path, inner, number, array, boolean, object))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("type("));
    assert!(compiled.source.contains("~= nil"));

    for pattern in [
        "crate::Other",
        "Some(crate::Other)",
        "Some(crate::Other(_))",
        "Some(crate::Other { value: _ })",
        "crate::Other { value: _ }",
    ] {
        let handler = format!(
            "fn handler(_ctx: &Context) -> Result<Vec<Op>> {{ let value = serde_json::from_str::<Value>(\"{{}}\").ok(); let flag = matches!(value, {pattern}); Ok(vec![Op::log(format!(\"{{}}\", flag))]) }}"
        );
        let error =
            compile(&source(&handler), &options("handler", EntryKind::Service))
                .expect_err("unsupported pattern");
        assert!(
            error.message.contains("pattern"),
            "{pattern}: {}",
            error.message
        );
    }
}

#[test]
fn keeps_bodies_with_only_void_method_statements() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut values = vec![1];
    values.clear();
    Ok(Vec::new())
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("table.clear(values)"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("local function body_update()"),
        "{}",
        compiled.source
    );
}

#[test]
fn match_guards_read_pattern_bindings() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let text = match ctx.config.get("host").and_then(Value::as_str) {
        Some(value) if !value.is_empty() => value.to_owned(),
        _ => "127.0.0.1".to_owned(),
    };
    Ok(vec![Op::log(text)])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let source = &compiled.source;
    assert!(source.contains("local value = __t"), "{source}");
    assert!(source.contains("if (not (#value == 0)) then"), "{source}");
}

#[test]
fn compiles_nested_or_patterns() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let value = ctx.config.get("x").cloned().unwrap_or(Value::Null);
    let kind = match &value {
        Some(Value::Array(_) | Value::Object(_)) => "table",
        _ => "other",
    };
    let table = matches!(value, None | Some(Value::Null | Value::Bool(false)));
    Ok(vec![Op::log(format!("{kind} {table}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("\"table\""), "{}", compiled.source);
}

#[test]
fn compiles_result_patterns_and_err_arms() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let Ok(count) = serde_json::from_str::<i64>("1") else {
        return Ok(Vec::new());
    };
    if let Ok(Value::Object(value)) = serde_json::from_str::<Value>("{}") {
        let _ = value;
    }
    Ok(vec![Op::log(format!("{count}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("if not ("), "{}", compiled.source);
}

#[test]
fn compiles_match_indices_in_a_tuple_loop() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut found = 0;
    for (index, text) in "one\ntwo".match_indices('\n') {
        found += index + text.len();
    }
    Ok(vec![Op::log(format!("{found}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("string.find"),
        "{}",
        compiled.source
    );
}

#[test]
fn guards_tuple_loops_over_json_null() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let rows = serde_json::Value::Null;
    let mut total = 0;
    for (key, value) in rows {
        total += value.as_i64().unwrap_or(0) + key.len();
    }
    Ok(vec![Op::log(format!("{total}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let generated = &compiled.source;
    assert!(
        generated.contains("in pairs(type(__t1) == \"table\" and __t1 or {})"),
        "{generated}"
    );
}

#[test]
fn bails_on_luau_keywords_in_loop_and_tuple_bindings() {
    for handler in [
        r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let rows: Vec<i64> = vec![1];
    for end in rows {
        let _ = end;
    }
    Ok(vec![])
}
"#,
        r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let pair = (1, 2);
    let (end, value) = pair;
    Ok(vec![Op::log(format!("{}{}", end, value))])
}
"#,
        r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let value = ctx.state.get("value");
    let Some(end) = value else {
        return Ok(vec![]);
    };
    Ok(vec![Op::log(format!("{end}"))])
}
"#,
    ] {
        let error =
            compile(&source(handler), &options("handler", EntryKind::Service))
                .expect_err("bail");
        assert!(error.message.contains("Luau keyword"), "{}", error.message);
        assert!(error.line > 0 && error.column > 0, "{error:?}");
    }
}
