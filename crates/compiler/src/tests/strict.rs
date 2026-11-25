use crate::{EntryKind, StrictOptions, compile};

use super::{source, strict_options};

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
    assert!(output.contains("local config = CONFIG"), "{output}");
    assert!(
        output.contains("local translations = TRANSLATIONS"),
        "{output}"
    );
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
        output.contains("noctalia.pluginDataDir(__unpack({}))"),
        "{output}"
    );
    assert!(output.contains("noctalia.getSetting(__unpack("), "{output}");
    assert!(
        output.contains("(noctalia.outputs(__unpack({})) == true)"),
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
