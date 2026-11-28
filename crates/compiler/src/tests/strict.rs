use crate::{EntryKind, StrictOptions, compile};

use super::{source, strict_options};

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
