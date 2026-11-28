use crate::{EntryKind, Module, Options, StrictOptions, compile};

use super::{module_options, native_options, source, strict_options};

fn strict_compile(handler: &str) -> Result<String, String> {
    compile(
        &source(handler),
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
fn strict_rejects_a_thread_effect_at_its_source_position() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    std::thread::spawn(|| {});
    Ok(Vec::new())
}
"#;
    let error = strict_compile(handler).expect_err("thread effect");
    assert!(error.starts_with("widget.rs:6:5:"), "{error}");
    assert!(
        error.contains("effect `std::thread::spawn` has no API 57 mapping"),
        "{error}"
    );
}

#[test]
fn strict_rejects_an_ffi_effect_at_its_source_position() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    setenv("KEY", "value", 1);
    Ok(Vec::new())
}
"#;
    let error = strict_compile(handler).expect_err("ffi effect");
    assert!(error.starts_with("widget.rs:6:5:"), "{error}");
    assert!(error.contains("FFI calls have no API 57 member"), "{error}");
}

#[test]
fn auto_crosses_a_function_boundary_for_an_unmapped_effect() {
    let handler = r#"
fn tick() {
    std::thread::sleep(std::time::Duration::from_millis(1));
}

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    tick();
    Ok(vec![Op::log("ok")])
}
"#;
    let compiled = compile(
        &source(handler),
        &native_options("handler", EntryKind::Widget),
    )
    .expect("auto keeps compiling");

    assert_eq!(compiled.native.len(), 1, "{:?}", compiled.native);
    let native = &compiled.native[0];
    assert_eq!(native.function, "tick");
    assert!(
        native
            .reason
            .contains("main:6:5: effect `std::thread::sleep`"),
        "{}",
        native.reason
    );
    assert!(
        compiled.source.contains("__native_fn_main__tick()"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("local function body_render("),
        "{}",
        compiled.source
    );
}

#[test]
fn mapped_file_effects_stay_direct_calls_in_both_modes() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let text = std::fs::read_to_string("x")?;
    Ok(vec![Op::log(text)])
}
"#;
    let strict = strict_compile(handler).expect("strict maps readFile");
    assert!(strict.contains("noctalia.readFile(\"x\")"), "{strict}");

    let auto = compile(
        &source(handler),
        &native_options("handler", EntryKind::Widget),
    )
    .expect("auto maps readFile");
    assert!(auto.native.is_empty(), "{:?}", auto.native);
    assert!(
        auto.source.contains("noctalia.readFile(\"x\")"),
        "{}",
        auto.source
    );
}

#[test]
fn strict_points_at_the_module_that_defines_the_effect() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    work::tick();
    Ok(Vec::new())
}
"#;
    let modules = [Module {
        path: "work",
        source: "pub(crate) fn tick() { std::thread::sleep(std::time::Duration::from_millis(1)); }\n",
    }];
    let options = Options {
        handler: "handler",
        entry: "widget",
        state_key: "hello.state",
        kind: EntryKind::Widget,
        strings: &[],
        settings: &[],
        module: "main",
        modules: &modules,
        native: crate::NativeMode::Deny,
        strict: Some(StrictOptions {
            config: "local CONFIG = {}\n",
            translations: "local LANGUAGES = {}\nlocal TRANSLATIONS = {}\n",
            assets: &[],
        }),
    };
    let error = compile(&source(handler), &options)
        .expect_err("effect in another module");
    let rendered = error.render("main.rs");
    assert!(rendered.starts_with("work:1:24:"), "{rendered}");
    assert!(
        rendered.contains("effect `std::thread::sleep`"),
        "{rendered}"
    );
}

#[test]
fn deny_mode_rejects_unmapped_effects_without_strict_options() {
    let handler = r#"
fn tick() {
    std::thread::sleep(std::time::Duration::from_millis(1));
}

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    tick();
    Ok(Vec::new())
}
"#;
    let error = compile(
        &source(handler),
        &module_options("handler", EntryKind::Widget, &[]),
    )
    .expect_err("deny mode rejects the effect");
    assert!(
        error.message.contains("effect `std::thread::sleep`"),
        "{error}"
    );
}

#[test]
fn an_explicit_native_boundary_keeps_its_cache() {
    let handler = r#"
#[native(cache_ms = 30_000)]
fn tick() {
    std::thread::sleep(std::time::Duration::from_millis(1));
}

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    tick();
    Ok(vec![Op::log("ok")])
}
"#;
    let compiled = compile(
        &source(handler),
        &native_options("handler", EntryKind::Widget),
    )
    .expect("auto keeps compiling");
    assert_eq!(compiled.native.len(), 1, "{:?}", compiled.native);
    assert_eq!(compiled.native[0].cache_ms, 30_000);
}

#[test]
fn strict_rejects_a_pretty_printer_without_an_api_57_member() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let text = serde_json::to_string_pretty(&json!({ "a": 1 }))?;
    Ok(vec![Op::log(text)])
}
"#;
    let error = strict_compile(handler).expect_err("pretty printer");
    assert!(
        error.contains("API 57 has no pretty JSON encoder"),
        "{error}"
    );
}

#[test]
fn strict_emit_and_typed_ir_agree_on_mapped_and_native_effects() {
    for (call, member) in [
        ("std::fs::read(\"x\")", "noctalia.readFile"),
        ("std::env::var(\"KEY\")", "noctalia.getenv"),
        ("serde_json::to_string(json!(1))", "noctalia.json.encode"),
    ] {
        let handler = format!(
            "fn handler(_ctx: &Context) -> Result<Vec<Op>> {{ let _ = {call}; Ok(Vec::new()) }}"
        );
        strict_compile(&handler)
            .unwrap_or_else(|error| panic!("{call}: {error}"));
        let program = crate::ir::lower_strict(
            &source(&handler),
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
        .unwrap_or_else(|error| panic!("{call}: {error}"));
        assert!(
            program.effect_sites.iter().any(|site| {
                matches!(&site.effect, crate::ir::Effect::Host { path, .. } if path == member)
            }),
            "{call} is not a host effect in the IR: {:?}",
            program.effect_sites
        );
    }

    let handler = "fn handler(_ctx: &Context) -> Result<Vec<Op>> { let _ = std::fs::read_link(\"x\"); Ok(Vec::new()) }";
    assert!(
        strict_compile(handler).is_err(),
        "emitter rejects read_link"
    );
    assert!(
        crate::ir::lower_strict(
            &source(handler),
            &module_options("handler", EntryKind::Widget, &[]),
        )
        .is_err(),
        "IR rejects read_link"
    );
}
