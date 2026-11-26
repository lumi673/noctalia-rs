use super::*;

#[test]
fn native_result_flag_tracks_the_declared_return_type() {
    let handler = r#"
#[native]
fn load_rows() -> Result<Vec<String>> {
    Ok(Vec::new())
}

#[native]
fn now() -> i64 {
    1
}

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let rows = load_rows()?;
    let time = now();
    Ok(vec![Op::log(rows.join(",")), Op::log(format!("{time}"))])
}
"#;
    let compiled = compile(
        &source(handler),
        &native_options("handler", EntryKind::Widget),
    )
    .expect("compiles");

    let load = compiled
        .native
        .iter()
        .find(|item| item.function == "load_rows")
        .expect("Result native");
    let now = compiled
        .native
        .iter()
        .find(|item| item.function == "now")
        .expect("plain native");
    assert!(load.result, "{load:?}");
    assert!(!now.result, "{now:?}");
}

#[test]
fn auto_mode_splits_at_the_function_boundary() {
    let compiled = compile(
        &source(NATIVE_HELPER),
        &native_options("handler", EntryKind::Widget),
    )
    .expect("compiles");

    assert_eq!(compiled.native.len(), 1, "{:?}", compiled.native);
    let native = &compiled.native[0];
    assert_eq!(native.name, "main::load_rows");
    assert!(native.params.is_empty());
    assert!(native.reason.contains("std::fs"), "{}", native.reason);
    assert!(
        compiled.source.contains(
            "local function __native_fn_main__load__rows()\n  return __native.call(\"main::load_rows\", {}, 0, false, true)\nend"
        ),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("local function body_render("),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("__native.run(body_"),
        "{}",
        compiled.source
    );
    assert!(!compiled.source.contains("std::fs"), "{}", compiled.source);
    assert!(
        compiled.source.contains("local __native = (function()"),
        "{}",
        compiled.source
    );
}

#[test]
fn strict_mode_still_rejects_the_same_handler() {
    let error = compile(
        &source(NATIVE_HELPER),
        &options("handler", EntryKind::Widget),
    )
    .expect_err("strict rejects");
    assert!(error.message.contains("std::fs"), "{error}");
    assert!(!error.fatal);
}

#[test]
fn fully_compiled_handlers_never_mention_the_native_boundary() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::log(ctx.config_str("message").unwrap_or_default().into_owned())])
}
"#;
    let compiled = compile(
        &source(handler),
        &native_options("handler", EntryKind::Widget),
    )
    .expect("compiles");

    assert!(compiled.native.is_empty());
    for forbidden in ["__native", "coroutine", "require(", "bridge."] {
        assert!(
            !compiled.source.contains(forbidden),
            "{forbidden} leaked into a fully compiled entry:\n{}",
            compiled.source
        );
    }
}

#[test]
fn native_attribute_forces_a_stub_and_records_the_cache() {
    let handler = r#"
fn digest(value: String) -> String {
    let mut out = String::new();
    for character in value.chars() {
        out.push(character);
    }
    out
}

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let text = digest("a".to_owned());
    Ok(vec![Op::log(text)])
}
"#;
    let compiled =
        compile(
            &source(&handler.replace(
                "fn digest(",
                "#[native(cache_ms = 30_000)]\nfn digest(",
            )),
            &native_options("handler", EntryKind::Widget),
        )
        .expect("compiles");

    assert_eq!(compiled.native.len(), 1);
    assert_eq!(compiled.native[0].cache_ms, 30_000);
    assert_eq!(
        compiled.native[0]
            .params
            .iter()
            .map(|param| param.name.as_str())
            .collect::<Vec<_>>(),
        vec!["value"]
    );
    assert!(
        compiled.source.contains("{ value = value }, 30000"),
        "{}",
        compiled.source
    );
    assert!(
        compiled
            .source
            .contains("local function __native_fn_main__digest(value)"),
        "{}",
        compiled.source
    );
}

#[test]
fn luau_attribute_turns_a_split_into_a_fatal_error() {
    let handler = r#"
#[luau]
fn load_rows() -> Result<Vec<String>> {
    let text = std::fs::read_link("/tmp/rows")?.to_string_lossy().into_owned();
    Ok(vec![text])
}

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let rows = load_rows()?;
    Ok(vec![Op::log(rows.join(","))])
}
"#;
    let error = compile(
        &source(handler),
        &native_options("handler", EntryKind::Widget),
    )
    .expect_err("#[luau] fails the build");
    assert!(error.fatal, "{error:?}");
    assert!(error.message.contains("#[luau]"), "{error}");
}

#[test]
fn native_methods_with_self_are_rejected() {
    let receiver = r#"
struct Store {
    value: i64,
}
impl Store {
    fn load_rows(&self) -> Result<Vec<String>> {
        let text = std::fs::read_link("/tmp/rows")?.to_string_lossy().into_owned();
        Ok(vec![text])
    }
}

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let store = Store { value: 1 };
    let rows = store.load_rows()?;
    Ok(vec![Op::log(rows.join(","))])
}
"#;
    let error = compile(
        &source(receiver),
        &native_options("handler", EntryKind::Widget),
    )
    .expect_err("methods with self cannot cross the boundary");
    assert!(
        error.message.contains("cannot take `self`")
            || error.message.contains("not supported"),
        "{error}"
    );
}

#[test]
fn native_calls_inside_closures_are_rejected() {
    let handler = r#"
fn load_rows() -> Result<Vec<String>> {
    let text = std::fs::read_link("/tmp/rows")?.to_string_lossy().into_owned();
    Ok(vec![text])
}

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let callback = |_value: i64| -> i64 {
        let _ = load_rows();
        1
    };
    let rows = callback(1);
    Ok(vec![Op::log(format!("{rows}"))])
}
"#;
    let error = compile(
        &source(handler),
        &native_options("handler", EntryKind::Widget),
    )
    .expect_err("closures cannot yield");
    assert!(error.message.contains("closure"), "{error}");
}

#[test]
fn a_native_function_is_emitted_once_per_script() {
    let handler = r#"
fn load_rows() -> Result<Vec<String>> {
    let text = std::fs::read_link("/tmp/rows")?.to_string_lossy().into_owned();
    Ok(vec![text])
}

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "render" {
        let rows = load_rows()?;
        Ok(vec![Op::log(rows.join(","))])
    } else if ctx.event == "click" {
        let rows = load_rows()?;
        Ok(vec![Op::log(rows.join(","))])
    } else {
        Ok(Vec::new())
    }
}
"#;
    let compiled = compile(
        &source(handler),
        &native_options("handler", EntryKind::Widget),
    )
    .expect("compiles");
    assert_eq!(compiled.native.len(), 1);
    assert_eq!(
        compiled
            .source
            .matches("local function __native_fn_main__load__rows")
            .count(),
        1,
        "{}",
        compiled.source
    );
}

#[test]
fn native_reference_parameters_receive_owned_values() {
    let handler = r#"
fn parse(path: &str, items: &[String]) -> Result<Vec<String>> {
    let text = std::fs::read_link(path)?.to_string_lossy().into_owned();
    Ok(vec![format!("{text}{}", items.len())])
}

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let rows = parse("/tmp/x", &["a".to_owned()])?;
    Ok(vec![Op::log(rows.join(","))])
}
"#;
    let compiled = compile(
        &source(handler),
        &native_options("handler", EntryKind::Widget),
    )
    .expect("compiles");

    let params = &compiled.native[0].params;
    assert_eq!(params.len(), 2);
    assert_eq!(params[0].name, "path");
    assert_eq!(params[0].owned.as_deref(), Some("String"));
    assert_eq!(params[0].pass, crate::NativePass::Ref);
    assert_eq!(params[1].name, "items");
    assert_eq!(params[1].owned.as_deref(), Some("Vec<_>"));
    assert_eq!(params[1].pass, crate::NativePass::Slice);
}

#[test]
fn native_mutable_and_nested_reference_parameters_are_rejected() {
    let mutable = r#"
fn parse(path: &mut String) -> Result<Vec<String>> {
    path.push('x');
    let text = std::fs::read_link(path)?.to_string_lossy().into_owned();
    Ok(vec![text])
}

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let rows = parse(&mut "a".to_owned())?;
    Ok(vec![Op::log(rows.join(","))])
}
"#;
    let error = compile(
        &source(mutable),
        &native_options("handler", EntryKind::Widget),
    )
    .expect_err("mutable references lose writes");
    assert!(error.message.contains("&mut"), "{error}");
}

#[test]
fn native_reference_returns_are_serialized() {
    let handler = r#"
fn label() -> Result<&'static str> {
    let text = std::fs::read_link("/tmp/x")?.to_string_lossy().into_owned();
    let _ = text;
    Ok("ready")
}

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let text = label()?;
    Ok(vec![Op::log(text.to_owned())])
}
"#;
    let compiled = compile(
        &source(handler),
        &native_options("handler", EntryKind::Widget),
    )
    .expect("reference returns serialize");
    assert_eq!(compiled.native.len(), 1);
    assert!(compiled.native[0].result);
}

#[test]
fn native_functions_can_take_the_handler_context() {
    let handler = r#"
fn describe(ctx: &Context) -> Result<String> {
    let text = std::fs::read_link("/tmp/x")?.to_string_lossy().into_owned();
    Ok(format!("{}:{}", ctx.event, text))
}

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let line = describe(ctx)?;
    Ok(vec![Op::log(line)])
}
"#;
    let compiled = compile(
        &source(handler),
        &native_options("handler", EntryKind::Widget),
    )
    .expect("compiles");

    let native = &compiled.native[0];
    assert_eq!(native.params.len(), 1);
    assert_eq!(
        native.params[0].kind,
        crate::NativeParamKind::Context,
        "{native:?}"
    );
    assert!(
        compiled
            .source
            .contains("local function __native_fn_main__describe()\n  return __native.call(\"main::describe\", {}, 0, true, true)\nend"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains(
            "__native.run(body_events, { event = \"click\", payload = { axis = axis, steps = steps, starts_gesture = startsGesture } }"
        ) || compiled.source.contains("__native.run(body_render, { event = \"render\""),
        "{}",
        compiled.source
    );
    assert!(
        compiled
            .source
            .contains("local function snapshot(descriptor)"),
        "{}",
        compiled.source
    );
    assert!(
        compiled
            .source
            .contains("CONFIG_KEYS = { \"message\", \"name\" }"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("STATE_KEY = \"hello.state\""),
        "{}",
        compiled.source
    );
}

#[test]
fn native_optional_references_use_owned_optionals() {
    let handler = r#"
fn truthy(value: Option<&Value>) -> bool {
    let _ = std::fs::read_link("/tmp/x");
    value.is_some()
}

fn text(value: Option<&str>) -> String {
    let _ = std::fs::read_link("/tmp/x");
    value.unwrap_or("").to_owned()
}

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let flag = truthy(ctx.payload.get("value"));
    let label = text(ctx.payload.get("label").and_then(|item| item.as_str()));
    Ok(vec![Op::log(format!("{flag}{label}"))])
}
"#;
    let compiled = compile(
        &source(handler),
        &native_options("handler", EntryKind::Widget),
    )
    .expect("compiles");

    let truthy = compiled
        .native
        .iter()
        .find(|item| item.name == "main::truthy")
        .expect("truthy native");
    assert_eq!(truthy.params[0].owned, None);
    assert_eq!(truthy.params[0].pass, crate::NativePass::AsRef);

    let text = compiled
        .native
        .iter()
        .find(|item| item.name == "main::text")
        .expect("text native");
    assert_eq!(text.params[0].owned.as_deref(), Some("Option<String>"));
    assert_eq!(text.params[0].pass, crate::NativePass::AsDeref);
}

#[test]
fn a_private_module_helper_needs_crate_visibility_to_split() {
    let main = r#"
mod widget;
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::log(widget::load())])
}
"#;
    let widget = r#"
fn load() -> String {
    let text = std::fs::read_link("/tmp/x").map(|value| value.to_string_lossy().into_owned()).unwrap_or_default();
    text
}
"#;
    let modules = [Module {
        path: "widget",
        source: widget,
    }];
    let error = compile(
        &source(main),
        &native_options_with("handler", EntryKind::Widget, &modules),
    )
    .expect_err("private helpers cannot be called from the crate root");
    assert!(error.message.contains("pub(crate)"), "{error}");

    let public = widget.replace("fn load()", "pub(crate) fn load()");
    let modules = [Module {
        path: "widget",
        source: &public,
    }];
    let compiled = compile(
        &source(main),
        &native_options_with("handler", EntryKind::Widget, &modules),
    )
    .expect("visible helper splits");
    assert_eq!(compiled.native[0].name, "widget::load");
}

#[test]
fn strict_ignores_the_native_attribute() {
    let handler = r#"
#[native]
fn label(value: usize) -> String {
    format!("{value} items")
}

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::log(label(2))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Widget))
            .expect("strict compiles the function instead of crossing");
    assert!(compiled.native.is_empty());
    assert!(!compiled.source.contains("__native"), "{}", compiled.source);
}

#[test]
fn a_luau_requirement_is_not_swallowed_by_an_outer_split() {
    let handler = r#"
#[luau]
fn inner() -> Result<String> {
    let text = std::fs::read_link("/tmp/x")?.to_string_lossy().into_owned();
    Ok(text)
}

fn outer() -> Result<String> {
    let text = inner()?;
    Ok(text)
}

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::log(outer()?)])
}
"#;
    let error = compile(
        &source(handler),
        &native_options("handler", EntryKind::Widget),
    )
    .expect_err("the fatal requirement must reach the build");
    assert!(error.fatal, "{error:?}");
    assert!(error.message.contains("#[luau]"), "{error}");
}

#[test]
fn native_borrowed_container_and_path_parameters() {
    let handler = r#"
fn summary(items: &Vec<String>, path: Option<&Path>, buf: Option<&PathBuf>) -> String {
    let text = std::fs::read_link("/tmp/x").map(|value| value.to_string_lossy().into_owned()).unwrap_or_default();
    format!("{}{}{}{}", items.len(), path.is_some(), buf.is_some(), text)
}

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::log(summary(&vec!["a".to_owned()], None, None))])
}
"#;
    let compiled = compile(
        &source(handler),
        &native_options("handler", EntryKind::Widget),
    )
    .expect("compiles");
    let params = &compiled.native[0].params;
    assert_eq!(params[0].owned, None);
    assert_eq!(params[0].pass, crate::NativePass::Ref);
    assert_eq!(params[1].owned.as_deref(), Some("Option<PathBuf>"));
    assert_eq!(params[1].pass, crate::NativePass::AsDeref);
    assert_eq!(params[2].owned.as_deref(), Some("Option<PathBuf>"));
    assert_eq!(params[2].pass, crate::NativePass::AsRef);
}

#[test]
fn a_native_result_keeps_the_error_path_for_unwrap() {
    let handler = r#"
fn label() -> Result<String> {
    let text = std::fs::read_link("/tmp/x")?.to_string_lossy().into_owned();
    Ok(text)
}

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = label().unwrap_or_default();
    Ok(vec![Op::log(value)])
}
"#;
    let compiled = compile(
        &source(handler),
        &native_options("handler", EntryKind::Widget),
    )
    .expect("compiles");
    assert!(
        compiled.source.contains(
            "return __native.call(\"main::label\", {}, 0, false, true)"
        ),
        "{}",
        compiled.source
    );
    assert!(
        compiled
            .source
            .contains("local __t1, __t2 = __native_fn_main__label()"),
        "{}",
        compiled.source
    );
    assert!(
        compiled
            .source
            .contains("if __t2 ~= nil then __t3 = \"\" end"),
        "{}",
        compiled.source
    );
}

#[test]
fn a_tail_native_call_lands_in_the_parent_value() {
    let handler = r#"
#[native]
fn label() -> String {
    "hi".to_owned()
}

fn helper() -> String {
    label()
}

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = helper();
    Ok(vec![Op::log(value)])
}
"#;
    let compiled = compile(
        &source(handler),
        &native_options("handler", EntryKind::Widget),
    )
    .expect("compiles");
    let body = compiled
        .source
        .lines()
        .skip_while(|line| !line.starts_with("local function body_render"))
        .take_while(|line| *line != "end")
        .collect::<Vec<_>>()
        .join("\n");
    assert!(body.contains("__t1 = __native_fn_main__label()"), "{body}");
    assert!(body.contains("noctalia.log"), "{body}");
}

#[test]
fn a_tail_native_result_keeps_its_error() {
    let handler = r#"
#[native]
fn label() -> Result<String> {
    Ok("hi".to_owned())
}

fn helper() -> Result<String> {
    label()
}

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = helper()?;
    Ok(vec![Op::log(value)])
}
"#;
    let compiled = compile(
        &source(handler),
        &native_options("handler", EntryKind::Widget),
    )
    .expect("compiles");
    let body = compiled
        .source
        .lines()
        .skip_while(|line| !line.starts_with("local function body_render"))
        .take_while(|line| *line != "end")
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        body.contains(", __t3 = __native_fn_main__label()"),
        "{body}"
    );
    assert!(
        body.contains("if __t3 ~= nil then error(tostring(__t3)) end"),
        "{body}"
    );
    assert!(body.contains("__t1 = __t2"), "{body}");
    assert!(body.contains("noctalia.log(value)"), "{body}");
}
