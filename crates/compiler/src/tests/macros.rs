use super::*;

#[test]
fn compiles_a_service_with_ops_and_formatting() {
    let handler = r#"
fn service(ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    let name = ctx.config_str("name").unwrap_or("bench");
    ops.push(Op::SetInterval { ms: 1000 });
    ops.push(Op::log(format!("{} ready", name)));
    Ok(ops)
}
"#;
    let compiled =
        compile(&source(handler), &options("service", EntryKind::Service))
            .expect("compiles");

    assert!(compiled.source.contains("noctalia.setUpdateInterval(1000)"));
    assert!(compiled.source.contains("noctalia.log(name .. \" ready\")"));
    assert!(
        compiled.source.contains(
            "local name = (noctalia.getConfig(\"name\") or \"bench\")"
        )
    );
    assert!(!compiled.source.contains("body_load"));
    assert!(!compiled.source.contains("noctalia.state.watch(STATE_KEY"));
}

#[test]
fn compiles_event_matches_into_callbacks() {
    let handler = r#"
fn launcher(ctx: &Context) -> Result<Vec<Op>> {
    match ctx.event.as_str() {
        "query" | "search" => {
            let text = ctx.payload_str("text").unwrap_or_default();
            Ok(vec![Op::LauncherResults { text, rows: vec![] }])
        }
        "activate" => {
            let id = ctx.payload_str("id").unwrap_or_default();
            Ok(vec![Op::log(id)])
        }
        _ => Ok(Vec::new()),
    }
}
"#;
    let compiled =
        compile(&source(handler), &options("launcher", EntryKind::Launcher))
            .expect("compiles");

    assert!(compiled.source.contains("local function body_"));
    assert!(compiled.source.contains("launcher.setResults(text, {})"));
    assert!(compiled.source.contains("noctalia.log(id)"));
    assert!(compiled.source.contains("function update() end"));
    assert!(
        compiled
            .source
            .contains("function onQuery(text) last_query = text; body_query(")
    );
    assert!(compiled.source.contains("function onSearch(text) body_"));
    assert!(
        compiled
            .source
            .contains("function onActivate(id) body_activate(")
    );
    assert!(compiled.source.contains("launcher.setResults(text, {})"));
    assert!(compiled.source.contains("last_query"));
    assert_eq!(compiled.events, vec!["activate", "query", "search"]);
}

#[test]
fn compiles_json_parsing_and_result_forms() {
    let handler = r#"
use serde_json::Value;
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let data = serde_json::from_str::<Value>("{\"n\":1}")?;
    let n = data.get("n").and_then(Value::as_i64).unwrap_or_default();
    let maybe = serde_json::from_str::<Value>("null").ok();
    let fallback = serde_json::from_str::<Value>("nope").unwrap_or(Value::Null);
    let text = serde_json::to_string(&data).unwrap_or_default();
    Ok(vec![Op::log(format!("{} {} {} {}", n, maybe.is_some(), fallback.is_null(), text))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("json.decode("));
    assert!(compiled.source.contains("error(tostring(__t"));
    assert!(compiled.source.contains("json.encode("));
    assert!(compiled.source.contains("nil"));
}

#[test]
fn compiles_json_result_patterns() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = match serde_json::from_str::<serde_json::Value>("{}") {
        Ok(data) => data,
        Err(_) => serde_json::Value::Null,
    };
    if let Ok(data) = serde_json::from_str::<serde_json::Value>("[]") {
        let _ = data;
    }
    let Ok(exact) = serde_json::from_str::<serde_json::Value>("null") else {
        return Ok(Vec::new());
    };
    Ok(vec![Op::log(value.to_string()), Op::log(exact.to_string())])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("if ((__t2 == nil)) then"));
    assert!(compiled.source.contains("if not (((__t6 == nil))) then"));
}

#[test]
fn bails_on_json_struct_parsing() {
    let handler = r#"
struct Rpc { value: i64 }
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = serde_json::from_str::<Rpc>("{}")?;
    let _ = value;
    Ok(Vec::new())
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(error.message.contains("JSON parsing into `Rpc`"));
    assert!(error.line > 0 && error.column > 0);
}

#[test]
fn bails_on_unsupported_json_null_and_op_structs() {
    for handler in [
        "fn handler(_ctx: &Context) -> Result<Vec<Op>> { let _ = json!({ \"a\": null }); Ok(vec![]) }",
        "fn handler(_ctx: &Context) -> Result<Vec<Op>> { let _ = json!([null]); Ok(vec![]) }",
        "fn handler(_ctx: &Context) -> Result<Vec<Op>> { Ok(vec![Op::Nope { value: 1 }]) }",
    ] {
        let error =
            compile(&source(handler), &options("handler", EntryKind::Service))
                .expect_err("unsupported");
        assert!(!error.message.is_empty());
    }
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let _ = json!(null);
    Ok(Vec::new())
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(!compiled.source.contains("noctalia.log"));
}

#[test]
fn compiles_format_escapes_named_arguments_and_radix() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = 42;
    let escaped = format!("{{literal}} and }}");
    let named = format!("{value}", value = value);
    let radix = i64::from_str_radix("ff", 16)?;
    Ok(vec![Op::log(format!("{} {} {}", escaped, named, radix))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("tonumber(\"ff\", 16)"));
    assert!(compiled.source.contains("{literal}"));
}

#[test]
fn bails_on_bad_format_macro_and_translation_shapes() {
    for handler in [
        "fn handler(_ctx: &Context) -> Result<Vec<Op>> { let text = format!(\"{}\"); Ok(vec![Op::log(text)]) }",
        "fn handler(_ctx: &Context) -> Result<Vec<Op>> { let text = format!(\"{missing}\"); Ok(vec![Op::log(text)]) }",
        "fn handler(_ctx: &Context) -> Result<Vec<Op>> { let bad = todo!(); let _ = bad; Ok(vec![]) }",
        "fn handler(ctx: &Context) -> Result<Vec<Op>> { let text = ctx.tr(\"key\", \"bad\"); Ok(vec![Op::log(text)]) }",
        "fn handler(ctx: &Context) -> Result<Vec<Op>> { let text = ctx.tr(\"key\", &[1]); Ok(vec![Op::log(text)]) }",
        "fn handler(ctx: &Context) -> Result<Vec<Op>> { let text = ctx.tr(\"key\", &[()]); Ok(vec![Op::log(text)]) }",
        "fn handler(ctx: &Context) -> Result<Vec<Op>> { let text = ctx.tr(\"key\", &[(\"only\",)]); Ok(vec![Op::log(text)]) }",
    ] {
        let error =
            compile(&source(handler), &options("handler", EntryKind::Service))
                .expect_err("unsupported shape");
        assert!(!error.message.is_empty(), "{handler}");
    }
}

#[test]
fn compiles_format_specifiers() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = 3.14159;
    let count = 7;
    let label = "hi";
    Ok(vec![Op::log(format!("{value:.2} {count:03} {label:>4} {count}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let generated = &compiled.source;
    assert!(
        generated.contains("string.format(\"%.2f\", value)"),
        "{generated}"
    );
    assert!(
        generated.contains("string.format(\"%03d\", count)"),
        "{generated}"
    );
    assert!(
        generated.contains("string.format(\"%4s\", label)"),
        "{generated}"
    );

    let bail = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let count = 7;
    Ok(vec![Op::log(format!("{count:x}"))])
}
"#;
    let error = compile(&source(bail), &options("handler", EntryKind::Service))
        .expect_err("bails");
    assert!(
        error.message.contains("format specifier"),
        "{}",
        error.message
    );
}

#[test]
fn compiles_debug_and_alignment_specifiers() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let count = 7;
    let label = "hi";
    Ok(vec![Op::log(format!("{count:?} {label:<4} {label:^4}"))])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("centering bails");
    assert!(
        error.message.contains("format specifier"),
        "{}",
        error.message
    );

    let simple = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let count = 7;
    let label = "hi";
    Ok(vec![Op::log(format!("{count:?} {label:<4}"))])
}
"#;
    let compiled =
        compile(&source(simple), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("string.format(\"%-4s\", label)"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("tostring(count)"),
        "{}",
        compiled.source
    );
}

#[test]
fn bails_on_debug_specifiers_for_strings() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let label = "hi";
    Ok(vec![Op::log(format!("{label:?}"))])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(
        error.message.contains("format specifier"),
        "{}",
        error.message
    );
}

#[test]
fn bails_on_debug_specifiers_for_json_values() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let value = ctx.config.get("value").cloned();
    Ok(vec![Op::log(format!("{value:?}"))])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(error.message.contains("String(\"x\")"), "{}", error.message);
    assert!(error.line > 0 && error.column > 0, "{error:?}");
}

#[test]
fn compiles_sign_and_alignment_specifiers_for_numbers() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let count = 7;
    Ok(vec![Op::log(format!("{count:+} {count:<5} {count:>5}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let emitted = &compiled.source;
    assert!(
        emitted.contains("string.format(\"%+d\", count)"),
        "{emitted}"
    );
    assert!(
        emitted.contains("string.format(\"%-5d\", count)"),
        "{emitted}"
    );
    assert!(
        emitted.contains("string.format(\"%5d\", count)"),
        "{emitted}"
    );

    let signed = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let label = "hi";
    Ok(vec![Op::log(format!("{label:+}"))])
}
"#;
    let error =
        compile(&source(signed), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(
        error.message.contains("format specifier"),
        "{}",
        error.message
    );
}

#[test]
fn compiles_finite_json_numbers() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let number = serde_json::Number::from_f64(1.5)
        .map_or(serde_json::Value::Null, serde_json::Value::Number);
    Ok(vec![Op::log(format!("{number}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let generated = &compiled.source;
    let finite = "(function() local __value = 1.5; if __value == __value and __value ~= math.huge and __value ~= -math.huge then return __value end end)()";
    assert!(generated.contains(finite), "{generated}");
    assert!(generated.contains("~= nil then"), "{generated}");
}

#[test]
fn compiles_matches_with_a_guard() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = Some(3);
    let matched = matches!(value, Some(number) if number > 1);
    Ok(vec![Op::log(format!("{matched}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains(" and "), "{}", compiled.source);
    assert!(
        compiled.source.contains("local number"),
        "{}",
        compiled.source
    );
}
