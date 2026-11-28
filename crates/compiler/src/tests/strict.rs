use crate::{EntryKind, StrictOptions, compile};

use super::{options, source, strict_options};

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
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "render" {
        let name = ctx.payload_str("action").unwrap_or_default();
        let id = format!("row-{name}");
        let tree = ui::row(json!({
            "onHover": ui::call(&name),
            "onClick": ui::callback(&id),
        }), Vec::new());
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
fn strict_reuses_an_inlined_acceptance_result() {
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
    assert!(output.contains("local streaming = __t"), "{output}");
    assert!(!output.contains("local streaming = \n"), "{output}");
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

#[test]
fn strict_typed_callback_emits_a_stable_closure() {
    let handler = r#"
use noctalia_rs::Callback;

struct RowClicked;

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    if ctx.event == "render" {
        let callback = Callback::<RowClicked>::new();
        return Ok(vec![Op::http_callback(json!({"url": "x"}), &callback)]);
    }
    Ok(Vec::new())
}
"#;
    let output = compile_strict(handler, &[]).expect("compile");
    assert!(
        output.contains("(function(__captured) return function(...) __strict_callback(__captured, ...) end end)(\"RowClicked\")"),
        "{output}"
    );
    assert!(
        output.contains("function __strict_callback(id, ...)"),
        "{output}"
    );
    assert!(!output.contains("__host_callback"), "{output}");
}

#[test]
fn strict_typed_action_emits_a_named_plugin_global() {
    let handler = r#"
use noctalia_rs::Action;

struct Refresh;

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "action" {
        return Ok(vec![Op::log("action")]);
    }
    if ctx.event == "render" {
        let tree = ui::button(json!({ "onClick": Action::<Refresh>::new() }));
        return Ok(vec![Op::BarRender { tree }]);
    }
    Ok(Vec::new())
}
"#;
    let output = compile_strict(handler, &[]).expect("compile");
    let global =
        format!("onClick = \"{}\"", crate::strict::action_global("Refresh"));
    assert!(output.contains(&global), "{output}");
    assert!(
        output.contains(&format!(
            "function {}(...)",
            crate::strict::action_global("Refresh")
        )),
        "{output}"
    );
    assert!(!output.contains("__call ="), "{output}");
}

#[test]
fn strict_typed_callback_without_a_branch_fails_at_its_span() {
    let handler = r#"
use noctalia_rs::Callback;

struct RowClicked;

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "render" {
        let _ = Callback::<RowClicked>::new();
    }
    Ok(Vec::new())
}
"#;
    let error =
        compile_strict(handler, &[]).expect_err("no host_callback branch");
    assert!(
        error.contains("typed callback needs a host_callback branch"),
        "{error}"
    );
    assert!(error.contains("widget.rs:"), "{error}");
}

#[test]
fn strict_named_handles_keep_their_kinds() {
    let handler = r#"
use noctalia_rs::{Action, Callback};

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    if ctx.event == "action" {
        return Ok(vec![Op::log("action")]);
    }
    if ctx.event == "render" {
        let callback = Callback::named("row");
        let tree = ui::button(json!({ "onClick": Action::named("reloadNow") }));
        return Ok(vec![
            Op::http_callback(json!({}), &callback),
            Op::BarRender { tree },
        ]);
    }
    Ok(Vec::new())
}
"#;
    let output = compile_strict(handler, &[]).expect("compile");
    assert!(
        output.contains("(function(__captured) return function(...) __strict_callback(__captured, ...) end end)(\"row\")"),
        "{output}"
    );
    let global = crate::strict::action_global("reloadNow");
    assert!(output.contains(&format!("\"{global}\"")), "{output}");
    assert!(
        output.contains(&format!("function {global}(...)")),
        "{output}"
    );
}

#[test]
fn strict_typed_callback_reassignment_fails_closed() {
    let handler = r#"
use noctalia_rs::Callback;

struct First;
struct Second;

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    if ctx.event == "render" {
        let mut callback = Callback::<First>::new();
        callback = Callback::<Second>::new();
        return Ok(vec![Op::http_callback(json!({}), &callback)]);
    }
    Ok(Vec::new())
}
"#;
    let error = compile_strict(handler, &[]).expect_err("reassigned id");
    assert!(
        error.contains("typed callback id must resolve to one id"),
        "{error}"
    );
    assert!(error.contains("widget.rs:"), "{error}");
}

#[test]
fn strict_typed_callback_with_two_possible_ids_fails_closed() {
    let handler = r#"
use noctalia_rs::Callback;

struct First;
struct Second;

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    if ctx.event == "render" {
        let callback = if ctx.payload_str("flag").unwrap_or_default() == "on" {
            Callback::<First>::new()
        } else {
            Callback::<Second>::new()
        };
        return Ok(vec![Op::http_callback(json!({}), &callback)]);
    }
    Ok(Vec::new())
}
"#;
    let error = compile_strict(handler, &[]).expect_err("two different ids");
    assert!(
        error.contains("typed callback id must resolve to one id"),
        "{error}"
    );
    assert!(error.contains("widget.rs:"), "{error}");
}

#[test]
fn strict_typed_callback_reassignment_in_a_loop_fails_closed() {
    let handler = r#"
use noctalia_rs::Callback;

struct First;
struct Second;

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    if ctx.event == "render" {
        let mut callback = Callback::<First>::new();
        for index in 0..2 {
            let _ = index;
            callback = Callback::<Second>::new();
        }
        return Ok(vec![Op::http_callback(json!({}), &callback)]);
    }
    Ok(Vec::new())
}
"#;
    let error = compile_strict(handler, &[]).expect_err("loop reassignment");
    assert!(
        error.contains("typed callback id must resolve to one id"),
        "{error}"
    );
    assert!(error.contains("widget.rs:"), "{error}");
}

#[test]
fn strict_typed_callback_uses_the_library_spelling_for_references() {
    let handler = r#"
use noctalia_rs::Callback;

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    if ctx.event == "render" {
        let callback = Callback::<&str>::new();
        return Ok(vec![Op::http_callback(json!({}), &callback)]);
    }
    Ok(Vec::new())
}
"#;
    let output = compile_strict(handler, &[]).expect("compile");
    assert!(output.contains("end end)(\"&str\")"), "{output}");
}

#[test]
fn strict_typed_callback_rejects_an_unproven_type_alias() {
    let handler = r#"
use noctalia_rs::Callback;

struct RowClicked;
type Clicked = RowClicked;

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    if ctx.event == "render" {
        let callback = Callback::<Clicked>::new();
        return Ok(vec![Op::http_callback(json!({}), &callback)]);
    }
    Ok(Vec::new())
}
"#;
    let error = compile_strict(handler, &[]).expect_err("alias");
    assert!(error.contains("widget.rs:"), "{error}");
    assert!(error.contains("Callback"), "{error}");
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
        error.contains("method `restart` is not supported"),
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
fn strict_handle_methods_accept_evaluated_receivers() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    ctx.run(json!(["echo"]), Some("done"), None).stop();
    Ok(Vec::new())
}
"#;
    let output = compile_strict(handler, &[]).expect("compile");
    assert!(output.contains(").stop()"), "{output}");
}

#[test]
fn strict_handle_clone_keeps_the_same_handle() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    let handle = ctx.run(json!(["echo"]), Some("done"), None);
    let saved = handle.clone();
    saved.stop();
    Ok(Vec::new())
}
"#;
    let output = compile_strict(handler, &[]).expect("compile");
    assert!(output.contains("local saved = handle"), "{output}");
    assert!(output.contains("saved.stop()"), "{output}");
}

#[test]
fn strict_optional_handles_keep_optional_combinators_and_fail_closed() {
    let handler = r#"
use noctalia_rs::RunHandle;

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    let handle = ctx.run(json!(["echo"]), Some("done"), None);
    let maybe: Option<RunHandle> = Some(handle);
    let _ = maybe.map(|h| h.stop());
    Ok(Vec::new())
}
"#;
    let error = compile_strict(handler, &[]).expect_err("typed closure");
    assert!(error.contains("method `stop` is not supported"), "{error}");
    assert!(error.contains("widget.rs:"), "{error}");
}

#[test]
fn strict_handle_accepts_a_typed_callback() {
    let handler = r#"
use noctalia_rs::Callback;

struct Done;

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    let callback = Callback::<Done>::new();
    let handle = ctx.run(json!(["echo"]), Some(callback), None);
    handle.stop();
    Ok(Vec::new())
}
"#;
    let output = compile_strict(handler, &[]).expect("compile");
    assert!(output.contains("__strict_run_handle("), "{output}");
    assert!(output.contains("\"Done\""), "{output}");
    assert!(!output.contains("__host_callback"), "{output}");
}

#[test]
fn strict_handle_started_result_is_typed_as_a_boolean() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    let started = ctx.run(json!(["echo"]), Some("done"), None).started();
    return Ok(vec![Op::log(format!("{started:?}"))]);
}
"#;
    let output = compile_strict(handler, &[]).expect("compile");
    assert!(output.contains("noctalia.log("), "{output}");
}
