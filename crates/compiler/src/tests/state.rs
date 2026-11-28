use crate::{EntryKind, compile};

use super::{options, source, strict_options};
use crate::StrictOptions;

fn derive_source() -> &'static str {
    r#"
#[derive(State)]
#[state(key = "panel.counter")]
struct Counter {
    #[state(default = 2)]
    count: i64,
    #[state(default = false)]
    enabled: bool,
    label: Option<String>,
    #[state(skip)]
    cache: i64,
}
"#
}

#[test]
fn strict_state_derive_uses_a_default_table_and_direct_host_state() {
    let handler = format!(
        "{}{}",
        derive_source(),
        r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    let state = Counter::load(ctx);
    state.save(&mut ops);
    Ok(ops)
}
"#
    );
    let config = "local CONFIG = {}\n";
    let translations = "local LANGUAGES = {}\nlocal TRANSLATIONS = {}\n";
    let compiled = compile(
        &source(&handler),
        &strict_options(
            "handler",
            EntryKind::Widget,
            StrictOptions {
                config,
                translations,
                assets: &[],
            },
        ),
    )
    .expect("compiles");
    let output = compiled.source;
    assert!(
        output.contains("noctalia.state.get(\"panel.counter\")"),
        "{output}"
    );
    assert!(output.contains("[\"count\"] = 2"), "{output}");
    assert!(output.contains("[\"enabled\"] = false"), "{output}");
    assert!(!output.contains("else __t2[\"label\"] ="), "{output}");
    assert!(output.contains("__t2[\"cache\"] = 0"), "{output}");
    assert!(
        output.contains("noctalia.state.set(\"panel.counter\""),
        "{output}"
    );
}

#[test]
fn auto_state_derive_keeps_the_host_callback_marker() {
    let handler = format!(
        "{}{}",
        derive_source(),
        r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    if ctx.is_load_event() {
        Counter::watch(&mut ops, "changed");
    }
    if ctx.event == "host_callback" && ctx.payload_str("id").as_deref() == Some("changed") {
        ops.push(Op::log("changed".to_owned()));
    }
    Ok(ops)
}
"#
    );
    let compiled =
        compile(&source(&handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains(
            "noctalia.state.watch(\"panel.counter\", { __host_callback = \"changed\" })"
        ),
        "{}",
        compiled.source
    );
}

#[test]
fn strict_state_derive_uses_a_real_callback_function() {
    let handler = format!(
        "{}{}",
        derive_source(),
        r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    if ctx.is_load_event() {
        Counter::watch(&mut ops, "changed");
    }
    if ctx.event == "host_callback" && ctx.payload_str("id").as_deref() == Some("changed") {
        ops.push(Op::log("changed".to_owned()));
    }
    Ok(ops)
}
"#
    );
    let config = "local CONFIG = {}\n";
    let translations = "local LANGUAGES = {}\nlocal TRANSLATIONS = {}\n";
    let compiled = compile(
        &source(&handler),
        &strict_options(
            "handler",
            EntryKind::Service,
            StrictOptions {
                config,
                translations,
                assets: &[],
            },
        ),
    )
    .expect("compiles");
    assert!(
        compiled
            .source
            .contains("noctalia.state.watch(\"panel.counter\", "),
        "{}",
        compiled.source
    );
    assert!(
        !compiled.source.contains("__host_callback"),
        "{}",
        compiled.source
    );
}

#[test]
fn state_derive_defaults_the_key_to_snake_case() {
    let handler = r#"
#[derive(State)]
struct AppState {
    #[state(default = "idle")]
    mode: String,
}
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let state = AppState::load(ctx);
    Ok(vec![Op::log(state.mode)])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled
            .source
            .contains("noctalia.state.get(\"app_state\")"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("[\"mode\"] = \"idle\""),
        "{}",
        compiled.source
    );
}

#[test]
fn state_derive_keeps_empty_container_shapes() {
    let handler = r#"
#[derive(State)]
struct Lists {
    #[state(default = Vec::new())]
    items: Vec<i64>,
    #[state(default = std::collections::HashMap::new())]
    lookup: std::collections::HashMap<String, i64>,
}
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let state = Lists::defaults();
    Ok(vec![Op::log(format!("{}", state.items.len()))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains(
            "[\"items\"] = setmetatable({}, { __noctalia_array = true })"
        ),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("[\"lookup\"] = {}"),
        "{}",
        compiled.source
    );
}

#[test]
fn state_derive_reads_the_host_value_once() {
    let handler = format!(
        "{}{}",
        derive_source(),
        r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let state = Counter::load(ctx);
    Ok(vec![Op::log(format!("{}", state.count))])
}
"#
    );
    let compiled =
        compile(&source(&handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let bodies = compiled.source.matches("local function body_").count();
    assert_eq!(
        compiled
            .source
            .matches("noctalia.state.get(\"panel.counter\")")
            .count(),
        bodies,
        "{}",
        compiled.source
    );
}

#[test]
fn state_derive_keeps_false_and_omits_none() {
    let handler = format!(
        "{}{}",
        derive_source(),
        r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let state = Counter::defaults();
    Ok(vec![Op::log(format!("{}", state.count))])
}
"#
    );
    let compiled =
        compile(&source(&handler), &options("handler", EntryKind::Widget))
            .expect("compiles");
    assert!(
        compiled.source.contains("[\"enabled\"] = false"),
        "{}",
        compiled.source
    );
    assert!(
        !compiled.source.contains("[\"label\"] = nil"),
        "{}",
        compiled.source
    );
}

#[test]
fn state_derive_accepts_some_defaults_and_rejects_bare_ones() {
    let handler = r#"
#[derive(State)]
struct Maybe {
    #[state(default = Some(5))]
    count: Option<i64>,
}
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let state = Maybe::load(ctx);
    Ok(vec![Op::log(format!("{}", state.count.unwrap_or_default()))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("[\"count\"] = 5"),
        "{}",
        compiled.source
    );

    let bare = handler.replace("Some(5)", "5");
    let error =
        compile(&source(&bare), &options("handler", EntryKind::Service))
            .expect_err("bare Option default");
    assert!(
        error.message.contains("Option state default"),
        "{}",
        error.message
    );
    assert!(error.line > 0 && error.column > 0);
}

#[test]
fn state_derive_defaults_can_be_saved() {
    let handler = format!(
        "{}{}",
        derive_source(),
        r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    let state = Counter::defaults();
    state.save(&mut ops);
    Ok(ops)
}
"#
    );
    let compiled =
        compile(&source(&handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled
            .source
            .contains("noctalia.state.set(\"panel.counter\""),
        "{}",
        compiled.source
    );
}

#[test]
fn state_derive_initializes_skipped_fields_without_saving_them() {
    let handler = format!(
        "{}{}",
        derive_source(),
        r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let state = Counter::load(ctx);
    Ok(vec![Op::log(format!("{}", state.cache))])
}
"#
    );
    let compiled =
        compile(&source(&handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("__t2[\"cache\"] = 0"),
        "{}",
        compiled.source
    );
    assert!(
        !compiled.source.contains("[\"cache\"] = __t"),
        "{}",
        compiled.source
    );
}

#[test]
fn state_derive_guards_stored_values_by_type() {
    let handler = format!(
        "{}{}",
        derive_source(),
        r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let state = Counter::load(ctx);
    Ok(vec![Op::log(format!("{}", state.count))])
}
"#
    );
    let compiled =
        compile(&source(&handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains(
            "type(__t1[\"count\"]) == \"number\" and __t1[\"count\"] % 1 == 0"
        ),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains(
            "type(__t1[\"enabled\"]) == \"boolean\" and __t1[\"enabled\"] ~= nil"
        ),
        "{}",
        compiled.source
    );
}

#[test]
fn state_derive_rejects_a_non_literal_default_with_position() {
    let handler = r#"
#[derive(State)]
struct Bad {
    #[state(default = std::i64::MAX)]
    count: i64,
}
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let state = Bad::defaults();
    Ok(vec![Op::log(format!("{}", state.count))])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Widget))
            .expect_err("unproven default");
    assert!(
        error.message.contains("integer literal"),
        "{}",
        error.message
    );
    assert!(error.line > 0 && error.column > 0);
}

#[test]
fn state_derive_rejects_an_integer_that_luau_cannot_hold_exactly() {
    let handler = r#"
#[derive(State)]
struct Wide {
    #[state(default = 9223372036854775808)]
    count: u64,
}
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let state = Wide::defaults();
    Ok(vec![Op::log(format!("{}", state.count))])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Widget))
            .expect_err("wide integer");
    assert!(
        error.message.contains("no exact Luau integer literal"),
        "{}",
        error.message
    );
    assert!(error.line > 0 && error.column > 0);
}

#[test]
fn state_derive_rejects_a_signed_default_for_an_unsigned_field() {
    let handler = r#"
#[derive(State)]
struct Bad {
    #[state(default = -1)]
    count: u64,
}
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let state = Bad::defaults();
    Ok(vec![Op::log(format!("{}", state.count))])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Widget))
            .expect_err("negative unsigned default");
    assert!(error.message.contains("does not fit"), "{}", error.message);
    assert!(error.line > 0 && error.column > 0);
}
