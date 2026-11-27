use crate::{EntryKind, StrictOptions, compile};

use super::{native_options, options, source, strict_options};

/// The metatable a `json!`/`Vec::new` empty array carries, so the runtime can
/// tell it from an empty object.
const EMPTY_ARRAY: &str = "setmetatable({}, { __noctalia_array = true })";

fn compile_strict(
    handler: &str,
    assets: &[(&str, u32)],
) -> Result<String, String> {
    let source = source(handler);
    let config = "local CONFIG = {}\n";
    let translations = "local LANGUAGES = {}\nlocal TRANSLATIONS = {}\n";
    compile(
        &source,
        &strict_options(
            "handler",
            EntryKind::Widget,
            StrictOptions {
                config,
                translations,
                assets,
            },
        ),
    )
    .map(|compiled| compiled.source)
    .map_err(|error| error.render("widget.rs"))
}

#[test]
fn strict_query_emits_a_direct_call_and_the_response_envelope() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    let response = format!("{}/q.json", ctx.data_dir.to_string_lossy());
    ops.push(Op::query("q", "noctalia", &["flatpakAppInstalled"], vec![json!("app")], response));
    Ok(ops)
}
"#;
    let output = compile_strict(handler, &[]).expect("compile");
    assert!(
        output.contains("noctalia.flatpakAppInstalled(__unpack({ \"app\" }))")
    );
    assert!(
        output.contains("noctalia.writeFile(response, noctalia.json.encode(")
    );
    assert!(!output.contains("Op::query"));
}

#[test]
fn strict_query_rejects_a_dynamic_path() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let path = vec!["flatpakAppInstalled"];
    let mut ops = Vec::new();
    ops.push(Op::query("q", "noctalia", &path, Vec::new(), String::new()));
    Ok(ops)
}
"#;
    let error = compile_strict(handler, &[]).expect_err("dynamic path");
    assert!(error.contains("literal path"), "{error}");
    assert!(error.contains("widget.rs:"), "{error}");
}

#[test]
fn strict_whole_config_and_translations_use_the_build_tables() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let config = ctx.config.clone();
    let translations = ctx.translations().clone();
    let _ = config;
    let _ = translations;
    Ok(vec![Op::log("ok")])
}
"#;
    let output = compile_strict(handler, &[]).expect("compile");
    assert!(output.contains("deep_clone(CONFIG)"), "{output}");
    assert!(output.contains("deep_clone(TRANSLATIONS)"), "{output}");
}

#[test]
fn strict_folds_a_chmod_on_a_declared_executable_asset() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let path = format!("/plugins/tools/tool");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = fs::metadata(&path) {
            let mut permissions = metadata.permissions();
            if permissions.mode() & 0o111 == 0 {
                permissions.set_mode(0o755);
                let _ = fs::set_permissions(&path, permissions);
            }
        }
    }
    Ok(vec![Op::log(path)])
}
"#;
    let output =
        compile_strict(handler, &[("tools/tool", 0o755)]).expect("compile");
    assert!(!output.contains("set_permissions"), "{output}");
    assert!(!output.contains("metadata"), "{output}");
    assert!(output.contains("noctalia.log(path)"), "{output}");
}

#[test]
fn strict_rejects_a_chmod_on_a_path_that_is_not_a_declared_asset() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let path = format!("/plugins/tools/other");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = fs::metadata(&path) {
            let mut permissions = metadata.permissions();
            if permissions.mode() & 0o111 == 0 {
                permissions.set_mode(0o755);
                let _ = fs::set_permissions(&path, permissions);
            }
        }
    }
    Ok(vec![Op::log(path)])
}
"#;
    let error = compile_strict(handler, &[("tools/tool", 0o755)])
        .expect_err("non-asset path");
    assert!(error.contains("not a declared asset"), "{error}");
}

#[test]
fn strict_rejects_a_chmod_the_build_mode_cannot_satisfy() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let path = format!("/plugins/tools/tool");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = fs::metadata(&path) {
            let mut permissions = metadata.permissions();
            if permissions.mode() & 0o111 == 0 {
                permissions.set_mode(0o755);
                let _ = fs::set_permissions(&path, permissions);
            }
        }
    }
    Ok(vec![Op::log(path)])
}
"#;
    let error = compile_strict(handler, &[("tools/tool", 0o644)])
        .expect_err("non-executable asset");
    assert!(error.contains("does not include the requested"), "{error}");
}

#[test]
fn strict_notify_action_defines_the_named_global() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "update" {
        return Ok(vec![Op::notify_with_options(
            "T",
            "B",
            json!({ "actions": [{ "id": "open", "label": "Open" }], "onAction": "onChoice" }),
        )]);
    }
    if ctx.event == "action" {
        let name = ctx.payload_str("name").unwrap_or_default();
        return Ok(vec![Op::log(name)]);
    }
    Ok(Vec::new())
}
"#;
    let output = compile_strict(handler, &[]).expect("compile");
    assert!(
        output.contains("function onChoice(id, context)"),
        "{output}"
    );
}

#[test]
fn strict_host_path_rejects_a_dynamic_name() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let path = format!("now_{}", "ms");
    let value = ctx.host(&path);
    Ok(vec![Op::log(format!("{value:?}"))])
}
"#;
    let error = compile_strict(handler, &[]).expect_err("dynamic host path");
    assert!(error.contains("literal path"), "{error}");
}

#[test]
fn strict_bytes_use_the_byte_safe_form() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let bytes = ByteBuf::new("raw".to_owned().into_bytes());
    let mut ops = Vec::new();
    ops.push(Op::copy_bytes(ByteBuf::new(vec![1u8, 2u8]), "application/octet-stream"));
    ops.push(Op::write_file_bytes("/tmp/x", bytes));
    ops.push(Op::http_bytes(json!({ "url": "x" }), ByteBuf::new(vec![3u8]), Some("done")));
    let payload = ctx.payload_bytes("raw").unwrap_or_default();
    let _ = ctx.write_file_bytes("/tmp/y", &payload);
    let _ = ctx.read_file_bytes_result("/tmp/y");
    let _ = ctx.payload_bytes("raw");
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    Ok(ops)
}
"#;
    let output = compile_strict(handler, &[]).expect("compile");
    assert!(output.contains("noctalia.copyToClipboard("), "{output}");
    assert!(output.contains("string.char(1, 2)"), "{output}");
    assert!(output.contains("string.char(3)"), "{output}");
    assert!(
        output.contains("noctalia.writeFile(\"/tmp/x\","),
        "{output}"
    );
    assert!(output.contains("__r.body ="), "{output}");
}

#[test]
fn strict_bytes_and_utf8_text_are_raw_luau_strings() {
    let handler = r#"
use noctalia_rs::{Bytes, Utf8Text};

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let raw = Bytes::new(vec![0u8, 255u8]);
    let text = Utf8Text::from_bytes(Bytes::new(vec![104u8]))?;
    let mut ops = Vec::new();
    ops.push(Op::copy_bytes(raw, "application/octet-stream"));
    ops.push(Op::log(text.as_str()));
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    Ok(ops)
}
"#;
    let output = compile_strict(handler, &[]).expect("compile");
    assert!(output.contains("string.char(0, 255)"), "{output}");
    assert!(output.contains("string.char(104)"), "{output}");
    assert!(output.contains("__check_utf8("), "{output}");
    assert!(
        output.contains("local function __check_utf8(value)"),
        "{output}"
    );
    assert!(
        !output.contains("__noctalia_rs_bytes"),
        "a marker crossed the strict ABI: {output}"
    );
}

#[test]
fn strict_rejects_an_out_of_range_byte_literal_at_its_span() {
    let handler = r#"
use noctalia_rs::Bytes;

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let raw = Bytes::new(vec![256u8]);
    Ok(vec![Op::log(raw.len().to_string())])
}
"#;
    let error = compile_strict(handler, &[]).expect_err("range");
    let span = error.strip_prefix("widget.rs:").expect("file:line:column");
    let mut numbers = span.split(':');
    let line = numbers
        .next()
        .expect("line")
        .parse::<usize>()
        .expect("number");
    let column = numbers
        .next()
        .expect("column")
        .parse::<usize>()
        .expect("number");
    assert!(line > 0 && column > 0, "{error}");
    assert!(error.contains("outside 0..=255"), "{error}");
    assert!(!error.contains("string.char(256)"), "{error}");
}

#[test]
fn strict_bytes_reject_json_encoding_and_validate_as_str() {
    let encode = r#"
use noctalia_rs::Bytes;

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let raw = Bytes::new(vec![255u8]);
    Ok(vec![Op::log(serde_json::to_string(&raw)?)])
}
"#;
    let error = compile_strict(encode, &[]).expect_err("no byte JSON");
    assert!(
        error.contains("UTF-8 validation"),
        "missing file:line:column: {error}"
    );
    assert!(error.contains("widget.rs:"), "{error}");
    assert!(!error.contains("__noctalia_rs_bytes"), "{error}");

    let checked = r#"
use noctalia_rs::Bytes;

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let raw = Bytes::new(vec![255u8]);
    Ok(vec![Op::log(raw.as_str().unwrap_or("fallback"))])
}
"#;
    let output = compile_strict(checked, &[]).expect("compile");
    assert!(output.contains("__check_utf8("), "{output}");
    assert!(
        output.contains("if __error ~= nil then return nil end"),
        "{output}"
    );
}

#[test]
fn strict_utf8_text_accepts_byte_array_literals() {
    let handler = r#"
use noctalia_rs::Utf8Text;

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let text = Utf8Text::try_from(&[104u8]).unwrap();
    Ok(vec![Op::log(text.as_str())])
}
"#;
    let output = compile_strict(handler, &[]).expect("compile");
    assert!(output.contains("string.char(104)"), "{output}");
    assert!(output.contains("__check_utf8("), "{output}");
    assert!(
        !output.contains("__check_utf8({ 104 })"),
        "a byte array became a table: {output}"
    );
}

#[test]
fn strict_json_macro_rejects_nested_bytes() {
    let handler = r#"
use noctalia_rs::Bytes;

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = json!({ "body": Bytes::new(vec![255u8]) });
    Ok(vec![Op::log(value.to_string())])
}
"#;
    let error = compile_strict(handler, &[]).expect_err("nested bytes");
    assert!(error.contains("widget.rs:"), "{error}");
    assert!(error.contains("cannot be placed in json!"), "{error}");
    assert!(!error.contains("__noctalia_rs_bytes"), "{error}");
}

#[test]
fn strict_utf8_text_supports_exact_string_searches() {
    let handler = r#"
use noctalia_rs::Utf8Text;

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let text = Utf8Text::new("hi");
    let at = text.find("h").unwrap_or_default();
    let last = text.rfind("i").unwrap_or_default();
    let _ = text.split_once("i");
    let _ = text.rsplit("i");
    Ok(vec![Op::log(format!("{at}{last}"))])
}
"#;
    let output = compile_strict(handler, &[]).expect("compile");
    assert!(output.contains("string.find("), "{output}");
    assert!(output.contains("string.sub("), "{output}");
}

#[test]
fn strict_acceptance_methods_report_booleans() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.run_stream_start(json!(["tail", "-f"]), "lines") {
        return Ok(vec![Op::log("started")]);
    }
    let timeout = Some(250i64);
    if ctx.run_async_start(json!(["echo", "hi"]), "done", timeout) {
        return Ok(Vec::new());
    }
    let http = ctx.http_start(json!({ "url": "x" }), "response");
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    Ok(vec![Op::log(http.to_string())])
}
"#;
    let output = compile_strict(handler, &[]).expect("compile");
    assert!(output.contains("local __t"), "{output}");
    assert!(output.contains("= noctalia.runStream("), "{output}");
    assert!(output.contains("= noctalia.runAsync("), "{output}");
    assert!(output.contains("= noctalia.http("), "{output}");
}

#[test]
fn strict_query_variants_resolve_direct_members() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let (dir, error) = _ctx.query_pair("noctalia", &["pluginDataDir"], Vec::new());
    let (value, failure) = _ctx.query_response("noctalia", &["getSetting"], vec![json!("shell.lang")]);
    let dark = _ctx.query_bool("noctalia", &["outputs"], Vec::new());
    let _ = (dir, error, value, failure);
    Ok(vec![Op::log(dark.to_string())])
}
"#;
    let output = compile_strict(handler, &[]).expect("compile");
    assert!(
        output.contains(&format!(
            "noctalia.pluginDataDir(__unpack({EMPTY_ARRAY}))"
        )),
        "{output}"
    );
    assert!(output.contains("noctalia.getSetting(__unpack("), "{output}");
    assert!(
        output.contains(&format!(
            "(noctalia.outputs(__unpack({EMPTY_ARRAY})) == true)"
        )),
        "{output}"
    );
    assert!(output.contains("return { ok = true, value ="), "{output}");
}

#[test]
fn strict_dynamic_ui_values_become_closures() {
    let handler = r#"
use noctalia_rs::{Action, CallbackId, Props};
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "render" {
        let name = ctx.payload_str("action").unwrap_or_default();
        let id = format!("row-{name}");
        let props = Props::new()
            .action("onHover", Action::dynamic(&name))
            .callback("onClick", CallbackId::new(&id));
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
    let output = compile_strict(handler, &[]).expect("compile");
    assert!(
        output.contains("__strict_action(__captured, ...)"),
        "{output}"
    );
    assert!(
        output.contains("__strict_callback(__captured, ...)"),
        "{output}"
    );
    assert!(
        output.contains("function __strict_action(name, ...)"),
        "{output}"
    );
}

#[test]
fn strict_calls_a_local_helper_for_acceptance_result() {
    let handler = r#"
fn start(ctx: &Context) -> bool {
    ctx.run_stream_start(json!(["tail", "-f"]), "stream")
}

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "update" {
        let streaming = start(ctx);
        return Ok(vec![Op::log(streaming.to_string())]);
    }
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    Ok(Vec::new())
}
"#;
    let output = compile_strict(handler, &[]).expect("compile");
    assert!(
        output.contains("local streaming = __noctalia_helper_"),
        "{output}"
    );
    assert!(
        output.contains("__noctalia_helper_")
            && output.contains(" = function(__arg0)"),
        "{output}"
    );
    assert!(output.contains("= noctalia.runStream("), "{output}");
}

#[test]
fn strict_rejects_a_notify_action_that_collides_with_a_handler() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "update" {
        return Ok(vec![Op::notify_with_options("T", "B", json!({ "onAction": "update" }))]);
    }
    if ctx.event == "action" {
        return Ok(vec![Op::log("action")]);
    }
    Ok(Vec::new())
}
"#;
    let error = compile_strict(handler, &[]).expect_err("collision");
    assert!(
        error.contains("collides with a generated handler"),
        "{error}"
    );
}

#[test]
fn strict_chmod_needs_a_whole_asset_path() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let path = "/plugins/tools/tool.bak";
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = fs::metadata(path) {
            let mut permissions = metadata.permissions();
            if permissions.mode() & 0o111 == 0 {
                permissions.set_mode(0o755);
                let _ = fs::set_permissions(path, permissions);
            }
        }
    }
    Ok(vec![Op::log(path)])
}
"#;
    let error = compile_strict(handler, &[("tools/tool", 0o755)])
        .expect_err("prefix is not the asset");
    assert!(error.contains("not a declared asset"), "{error}");
}

fn compile_auto(handler: &str) -> Result<String, String> {
    compile(&source(handler), &options("handler", EntryKind::Widget))
        .map(|compiled| compiled.source)
        .map_err(|error| error.render("widget.rs"))
}

#[test]
fn strict_run_handle_uses_api57_and_stops_the_callback() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    let handle = ctx.run(json!(["echo", "hi"]), Some("done"), Some(250i64));
    if handle.started() {
        handle.stop();
    }
    Ok(Vec::new())
}
"#;
    let output = compile_strict(handler, &[]).expect("compile");
    assert!(output.contains("__strict_run_handle"), "{output}");
    assert!(output.contains("noctalia.runAsync("), "{output}");
    assert!(output.contains("handle.started()"), "{output}");
    assert!(output.contains("handle.stop()"), "{output}");
    assert!(output.contains("250"), "{output}");
    assert!(!output.contains("__host_callback"), "{output}");
    assert!(!output.contains("__noctalia_rs"), "{output}");
    assert!(!output.contains("onHostCallback"), "{output}");
}

#[test]
fn strict_handles_lower_every_async_start_directly() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    let run = ctx.run(json!(["echo"]), Some("run"), None);
    let http = ctx.http(json!({ "url": "x" }), "http");
    let stream = ctx.http_stream(json!({ "url": "x" }), "line", "close");
    let watch = ctx.state_watch("watched", "changed");
    run.stop();
    http.stop();
    stream.stop();
    watch.stop();
    Ok(Vec::new())
}
"#;
    let output = compile_strict(handler, &[]).expect("compile");
    for helper in [
        "__strict_run_handle",
        "__strict_http_handle",
        "__strict_http_stream_handle",
        "__strict_state_watch_handle",
    ] {
        assert!(output.contains(helper), "{helper} missing: {output}");
    }
    for member in [
        "noctalia.runAsync(",
        "noctalia.http(",
        "noctalia.httpStream(",
        "state.watch(",
    ] {
        assert!(output.contains(member), "{member} missing: {output}");
    }
    assert!(!output.contains("__noctalia_rs"), "{output}");
    assert!(!output.contains("__host_callback"), "{output}");
}

#[test]
fn strict_handle_timeout_keeps_the_exact_integer_and_none() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    let wide = ctx.run(json!(["echo"]), Some("wide"), Some(9223372036854775807i64));
    let none = ctx.run(json!(["echo"]), Some("none"), None);
    wide.stop();
    none.stop();
    Ok(Vec::new())
}
"#;
    let output = compile_strict(handler, &[]).expect("compile");
    assert!(
        output.contains("\"wide\", 9223372036854775807)"),
        "{output}"
    );
    assert!(output.contains("\"none\", nil)"), "{output}");
}

#[test]
fn strict_handle_callback_needs_a_host_branch() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let _handle = ctx.http(json!({ "url": "x" }), "response");
    Ok(Vec::new())
}
"#;
    let error = compile_strict(handler, &[]).expect_err("missing branch");
    assert!(error.contains("host_callback branch"), "{error}");
    assert!(error.contains("widget.rs:"), "{error}");
    let line = error
        .split("widget.rs:")
        .nth(1)
        .and_then(|rest| rest.split(':').next())
        .and_then(|value| value.parse::<usize>().ok())
        .expect("line");
    let column = error
        .split("widget.rs:")
        .nth(1)
        .and_then(|rest| rest.split(':').nth(1))
        .and_then(|value| value.parse::<usize>().ok())
        .expect("column");
    assert!(line > 0 && column > 0, "{error}");
}

#[test]
fn strict_handle_rejects_an_unknown_method() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    let handle = ctx.state_watch("watched", "changed");
    handle.restart();
    Ok(Vec::new())
}
"#;
    let error = compile_strict(handler, &[]).expect_err("unknown method");
    assert!(
        error.contains("handle method `restart` is not supported"),
        "{error}"
    );
    assert!(error.contains("widget.rs:"), "{error}");
}

#[test]
fn strict_handle_preserves_evaluation_order() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    let mut ops = Vec::new();
    ops.push(Op::log("before"));
    let handle = ctx.run(json!(["echo"]), Some("done"), None);
    ops.push(Op::log("after"));
    handle.stop();
    Ok(ops)
}
"#;
    let output = compile_strict(handler, &[]).expect("compile");
    let body = output
        .split("local function body_render()")
        .nth(1)
        .expect("body")
        .split("\nend")
        .next()
        .expect("body end");
    let before = body.find("noctalia.log(\"before\")").expect("before");
    let start = body.find("__strict_run_handle(").expect("start");
    let after = body.find("noctalia.log(\"after\")").expect("after");
    assert!(before < start && start < after, "{body}");
}

#[test]
fn auto_mode_rejects_strict_handles() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let _handle = ctx.state_watch("watched", "changed");
    Ok(Vec::new())
}
"#;
    let error = compile_auto(handler).expect_err("auto mode");
    assert!(error.contains("needs strict mode"), "{error}");
    assert!(error.contains("widget.rs:"), "{error}");
}

#[test]
fn strict_handle_methods_use_the_bound_luau_name() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    let end = ctx.run(json!(["echo"]), Some("done"), None);
    end.stop();
    Ok(Vec::new())
}
"#;
    let output = compile_strict(handler, &[]).expect("compile");
    assert!(output.contains("local __kw_end = "), "{output}");
    assert!(output.contains("__kw_end.stop()"), "{output}");
    assert!(!output.contains("\n  end.stop()"), "{output}");
}

#[test]
fn strict_handle_methods_work_on_helper_parameters() {
    let handler = r#"
use noctalia_rs::RunHandle;

fn stop_it(handle: RunHandle) {
    handle.stop();
}

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    let handle = ctx.run(json!(["echo"]), Some("done"), None);
    stop_it(handle);
    Ok(Vec::new())
}
"#;
    let output = compile_strict(handler, &[]).expect("compile");
    assert!(output.contains(".stop()"), "{output}");
    assert!(output.contains("__strict_run_handle("), "{output}");
}

#[test]
fn strict_json_object_omits_absent_keys() {
    let handler = r#"
use serde_json::{Value, json};
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let maybe: Option<i64> = None;
    let value = json!({ "present": Some(7), "missing": None, "maybe": maybe });
    Ok(vec![Op::log(serde_json::to_string(&value)?)])
}
"#;
    let output = compile_strict(handler, &[]).expect("compile");
    assert!(!output.contains("[\"missing\"]"), "{output}");
    assert!(output.contains("~= nil then"), "{output}");
    assert!(output.contains("[\"maybe\"]"), "{output}");
}

#[test]
fn strict_json_arrays_reject_absent_slots() {
    let handler = r#"
use serde_json::json;
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let maybe: Option<i64> = Some(1);
    let value = json!([maybe]);
    Ok(vec![Op::log(serde_json::to_string(&value)?)])
}
"#;
    let error = compile_strict(handler, &[]).expect_err("absent slot");
    assert!(error.contains("widget.rs:"), "{error}");
    assert!(
        error.contains("a missing value cannot be stored"),
        "{error}"
    );
}

#[test]
fn strict_json_containers_reject_computed_null() {
    for handler in [
        r#"
use serde_json::{Value, json};
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let empty = Value::Null;
    let value = json!({ "a": empty });
    Ok(vec![Op::log(serde_json::to_string(&value)?)])
}
"#,
        r#"
use serde_json::{Value, json};
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let empty = Value::Null;
    let value = json!([empty]);
    Ok(vec![Op::log(serde_json::to_string(&value)?)])
}
"#,
    ] {
        let error = compile_strict(handler, &[]).expect_err("computed null");
        assert!(error.contains("widget.rs:"), "{error}");
        assert!(error.contains("json null"), "{error}");
    }
}

fn strict_source(source: &str) -> Result<String, String> {
    compile(
        source,
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

#[test]
fn strict_rejects_compat_imports_with_a_source_position() {
    let source = r#"use noctalia_rs::compat::Parts;
use noctalia_rs::{Context, Op, Result};

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::log("ok")])
}
"#;
    let error = strict_source(source).expect_err("compat import");
    assert!(error.contains("widget.rs:1:"), "{error}");
    assert!(error.contains("noctalia_rs::compat"), "{error}");

    let auto = compile(source, &native_options("handler", EntryKind::Widget));
    assert!(auto.is_ok(), "{:?}", auto.err());
}

#[test]
fn strict_rejects_helper_op_sink_methods() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    Ok(ctx.take_pending_ops())
}
"#;
    let error = compile_strict(handler, &[]).expect_err("compat method");
    assert!(error.contains("take_pending_ops"), "{error}");
    assert!(error.contains("widget.rs:"), "{error}");
}

#[test]
fn strict_rejects_coroutine_yields() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    use noctalia_rs::compat::yield_now;
    yield_now();
    Ok(Vec::new())
}
"#;
    let error = compile_strict(handler, &[]).expect_err("yield");
    assert!(error.contains("yield_now"), "{error}");
    assert!(error.contains("helper runtime"), "{error}");
    assert!(error.contains("widget.rs:"), "{error}");
}

#[test]
fn strict_rejects_raw_pointer_stashes() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let pointer: *const u8 = core::ptr::null();
    let _ = pointer;
    Ok(Vec::new())
}
"#;
    let error = compile_strict(handler, &[]).expect_err("raw pointer");
    assert!(error.contains("raw pointer"), "{error}");
    assert!(error.contains("widget.rs:"), "{error}");
}

#[test]
fn strict_rejects_compat_aliases_and_old_module_paths() {
    let cases = [
        "use noctalia_rs::compat as c;\nuse noctalia_rs::{Context, Op, Result};\nfn handler(_ctx: &Context) -> Result<Vec<Op>> { let _ = c::Parts; Ok(vec![Op::log(\"ok\")]) }\n",
        "use noctalia_rs as rs;\nuse noctalia_rs::{Context, Op, Result};\nfn handler(_ctx: &Context) -> Result<Vec<Op>> { let _ = rs::compat::Parts; Ok(vec![Op::log(\"ok\")]) }\n",
        "use noctalia_rs::context::Parts;\nuse noctalia_rs::{Context, Op, Result};\nfn handler(_ctx: &Context) -> Result<Vec<Op>> { let _: Option<Parts> = None; Ok(vec![Op::log(\"ok\")]) }\n",
        "use noctalia_rs::clock::ClockBudget;\nuse noctalia_rs::{Context, Op, Result};\nfn handler(_ctx: &Context) -> Result<Vec<Op>> { let _: Option<ClockBudget> = None; Ok(vec![Op::log(\"ok\")]) }\n",
        "use noctalia_rs::CoroutinePoll;\nuse noctalia_rs::{Context, Op, Result};\nfn handler(_ctx: &Context) -> Result<Vec<Op>> { let _: Option<CoroutinePoll> = None; Ok(vec![Op::log(\"ok\")]) }\n",
        "use noctalia_rs::yield_now;\nuse noctalia_rs::{Context, Op, Result};\nfn handler(_ctx: &Context) -> Result<Vec<Op>> { yield_now(); Ok(vec![Op::log(\"ok\")]) }\n",
        "use std::thread as t;\nuse noctalia_rs::{Context, Op, Result};\nfn handler(_ctx: &Context) -> Result<Vec<Op>> { t::yield_now(); Ok(vec![Op::log(\"ok\")]) }\n",
    ];
    for case in cases {
        let error = strict_source(case).expect_err("compat alias or old path");
        assert!(error.contains("helper runtime"), "{case}\n{error}");
    }
}

#[test]
fn strict_json_model_lowers_object_and_array() {
    let handler = r#"
use noctalia_rs::json::{Array, Object};
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut object = Object::new();
    object.insert("present", Some(7));
    object.insert("missing", None::<i64>);
    let mut array = Array::new();
    array.push(1);
    let text = serde_json::to_string(&object)?;
    let list = serde_json::to_string(&array)?;
    Ok(vec![Op::log(format!("{text} {list}"))])
}
"#;
    let output = compile_strict(handler, &[]).expect("compile");
    assert!(output.contains("table.insert"), "{output}");
    assert!(!output.contains("[\"missing\"]"), "{output}");
    assert!(output.contains("setmetatable"), "{output}");
}

#[test]
fn strict_json_model_rejects_absent_array_push() {
    let handler = r#"
use noctalia_rs::json::Array;
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let maybe: Option<i64> = Some(1);
    let mut array = Array::new();
    array.push(maybe);
    Ok(vec![Op::log(serde_json::to_string(&array)?)])
}
"#;
    let error = compile_strict(handler, &[]).expect_err("absent push");
    assert!(error.contains("widget.rs:"), "{error}");
    assert!(
        error.contains("a missing value cannot be stored"),
        "{error}"
    );
}

#[test]
fn strict_json_arrays_reject_optional_if_expressions() {
    let handler = r#"
use serde_json::json;
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let maybe: Option<i64> = Some(1);
    let value = json!([if ctx.now_ms > 0 { 2 } else { maybe }]);
    Ok(vec![Op::log(serde_json::to_string(&value)?)])
}
"#;
    let error =
        compile_strict(handler, &[]).expect_err("optional branch in array");
    assert!(error.contains("widget.rs:"), "{error}");
    assert!(
        error.contains("a missing value cannot be stored"),
        "{error}"
    );
}

#[test]
fn strict_json_handles_qualified_option_variants() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = json!({ "present": Option::Some(1), "missing": Option::None });
    Ok(vec![Op::log(serde_json::to_string(&value)?)])
}
"#;
    let output = compile_strict(handler, &[]).expect("compile");
    assert!(!output.contains("[\"missing\"]"), "{output}");
    assert!(output.contains("~= nil then"), "{output}");
}

#[test]
fn strict_allows_a_local_helper_named_yield_now() {
    let source = r#"use noctalia_rs::{Context, Op, Result};

fn yield_now(value: i64) -> i64 {
    value + 1
}

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = yield_now(1);
    Ok(vec![Op::log(value.to_string())])
}
"#;
    let output = strict_source(source).expect("local helper");
    assert!(output.contains("noctalia.log"), "{output}");
}

#[test]
fn strict_emits_recursive_helpers_as_local_functions() {
    let handler = r#"
fn count(n: i64) -> i64 {
    if n <= 0 { 0 } else { count(n - 1) + 1 }
}

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let value = count(12);
    Ok(vec![Op::log(value.to_string())])
}
"#;
    let output =
        compile_strict(handler, &[]).expect("recursive helper compiles");
    assert!(
        output.contains("__noctalia_helper_main__count")
            && output.contains(" = function(__arg0)"),
        "{output}"
    );
    assert!(!output.contains("recursive helper"), "{output}");
}

#[test]
fn strict_emits_mutually_recursive_helpers_with_forward_declarations() {
    let handler = r#"
fn even(n: i64) -> bool {
    if n == 0 { true } else { odd(n - 1) }
}
fn odd(n: i64) -> bool {
    if n == 0 { false } else { even(n - 1) }
}

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let value = even(10);
    Ok(vec![Op::log(value.to_string())])
}
"#;
    let output =
        compile_strict(handler, &[]).expect("mutual recursion compiles");
    assert!(
        output.contains("local __noctalia_helper_main__even"),
        "{output}"
    );
    assert!(
        output.contains("local __noctalia_helper_main__odd"),
        "{output}"
    );
    assert!(
        output.matches(" = function(__arg0)").count() >= 2,
        "{output}"
    );
}

#[test]
fn strict_helpers_ignore_inline_depth_and_call_limits() {
    let mut handler = String::new();
    for index in 0..12 {
        handler.push_str(&format!(
            "fn h{index}(ctx: &Context) -> i64 {{ h{}(ctx) }}\n",
            index + 1
        ));
    }
    handler.push_str("fn h12(_ctx: &Context) -> i64 { 1 }\n");
    handler.push_str(
        "fn handler(ctx: &Context) -> Result<Vec<Op>> {\n    let value = h0(ctx);\n",
    );
    for _ in 0..320 {
        handler.push_str("    let _ = h12(ctx);\n");
    }
    handler.push_str("    Ok(vec![Op::log(value.to_string())])\n}\n");
    let output =
        compile_strict(&handler, &[]).expect("deep helper chain compiles");
    assert!(!output.contains("depth limit"), "{output}");
    assert!(!output.contains("calls per body"), "{output}");
}

#[test]
fn strict_vec_ops_helper_works_in_expression_position() {
    let handler = r#"
fn build(ctx: &Context) -> Vec<Op> {
    let mut ops = Vec::new();
    ops.push(Op::log(ctx.event.as_str()));
    ops
}

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let ops = build(ctx);
    Ok(ops)
}
"#;
    let output = compile_strict(handler, &[]).expect("Vec<Op> helper compiles");
    assert!(
        output.contains("local ops = __noctalia_helper_main__build_"),
        "{output}"
    );
    assert!(output.contains("noctalia.log(event)"), "{output}");
}

#[test]
fn strict_result_vec_ops_helper_works_in_expression_position() {
    let handler = r#"
fn build(ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::log(ctx.event.as_str())])
}

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let ops = build(ctx)?;
    Ok(ops)
}
"#;
    let output =
        compile_strict(handler, &[]).expect("Result<Vec<Op>> helper compiles");
    assert!(output.contains("local __t"), "{output}");
    assert!(
        output.contains("__noctalia_helper_main__build_"),
        "{output}"
    );
    assert!(output.contains("noctalia.log(event)"), "{output}");
}

#[test]
fn strict_result_value_helper_uses_a_two_value_return() {
    let handler = r#"
fn value(ctx: &Context) -> Result<i64> {
    Ok(ctx.now_ms)
}

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let now = value(ctx)?;
    Ok(vec![Op::log(now.to_string())])
}
"#;
    let output = compile_strict(handler, &[]).expect("Result helper compiles");
    assert!(output.contains(", nil"), "{output}");
    assert!(output.contains("return noctalia.nowMs(), nil"), "{output}");
}

#[test]
fn strict_helper_reference_is_a_lua_function() {
    let handler = r#"
fn label() -> String { "label".to_owned() }

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let value = label();
    Ok(vec![Op::log(value)])
}
"#;
    let output = compile_strict(handler, &[]).expect("helper compiles");
    assert!(
        output.contains("local value = __noctalia_helper_main__label_"),
        "{output}"
    );
}

#[test]
fn strict_helper_bodies_share_the_outer_state_table() {
    let handler = r#"
fn state_count(ctx: &Context) -> i64 {
    ctx.state_value::<i64>("count").unwrap_or(0)
}

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let value = state_count(ctx);
    Ok(vec![Op::log(value.to_string())])
}
"#;
    let output = compile_strict(handler, &[]).expect("state helper compiles");
    let body_render = output
        .split("local function body_render()")
        .nth(1)
        .expect("render body")
        .split("\nend\n\nlocal function")
        .next()
        .expect("body end");
    assert_eq!(
        body_render.matches("local plugin_state =").count(),
        1,
        "{body_render}"
    );
    assert!(body_render.contains("plugin_state["), "{body_render}");
}

#[test]
fn strict_nested_result_ops_helper_propagates_its_error() {
    let handler = r#"
fn inner(_ctx: &Context) -> Result<Vec<Op>> {
    return Err("boom".to_owned());
}

fn outer(ctx: &Context) -> Result<Vec<Op>> {
    inner(ctx)
}

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let ops = outer(ctx)?;
    let _ = ops;
    Ok(vec![Op::log("done")])
}
"#;
    let output = compile_strict(handler, &[]).expect("nested helper compiles");
    assert!(output.contains("__noctalia_helper_main__outer"), "{output}");
    let outer = output
        .split("\n  __noctalia_helper_main__outer_")
        .nth(1)
        .expect("outer definition")
        .split("\n  end")
        .next()
        .expect("outer body");
    assert!(outer.contains("return nil, "), "{outer}");
    assert!(!outer.contains("error(tostring("), "{outer}");
}

#[test]
fn strict_recursive_helper_used_as_a_callback_value_is_local() {
    let handler = r#"
fn count(n: i64) -> i64 {
    if n <= 0 { 0 } else { count(n - 1) + 1 }
}

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let value = Some(4).map(count).unwrap_or_default();
    Ok(vec![Op::log(value.to_string())])
}
"#;
    let output =
        compile_strict(handler, &[]).expect("callback helper compiles");
    assert!(
        output.contains("__noctalia_helper_main__count_"),
        "{output}"
    );
    assert!(!output.contains("recursive helper"), "{output}");
}
