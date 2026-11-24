use crate::{EntryKind, Options, compile};

fn options<'a>(handler: &'a str, kind: EntryKind) -> Options<'a> {
    Options {
        handler,
        entry: "widget",
        state_key: "hello.state",
        kind,
        strings: &["message", "name"],
    }
}

fn source(handler: &str) -> String {
    format!(
        "use noctalia_rs::{{Context, Op, Result}};\nuse serde_json::json;\n\n{handler}\n"
    )
}

#[test]
fn compiles_a_hello_bar_handler() {
    let handler = r#"
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "render" || ctx.event == "click" {
        let message = ctx.config_str("message")
            .unwrap_or_default()
            .into_owned();
        Ok(vec![Op::BarText { text: message }])
    } else {
        Ok(Vec::new())
    }
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");

    assert!(compiled.source.contains("local function body_"));
    assert!(compiled.source.contains("function update() body_"));
    assert!(
        compiled.source.contains(
            "local message = (noctalia.getConfig(\"message\") or \"\")"
        )
    );
    assert!(compiled.source.contains("barWidget.setText(message)"));
    assert!(compiled.source.contains("function onClick() body_"));
    assert!(!compiled.source.contains("require("));
    assert!(!compiled.source.contains("bridge."));
    assert_eq!(compiled.events, vec!["click", "render"]);
    assert!(
        compiled.source.len() < 1_500,
        "hello world should stay small: {} bytes",
        compiled.source.len()
    );
}

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
fn compiles_ui_trees_to_table_literals() {
    let handler = r#"
fn panel(ctx: &Context) -> Result<Vec<Op>> {
    let label = ctx.state_value::<String>("label").unwrap_or_default();
    let tree = ui::column(
        json!({ "id": "root", "gap": 4 }),
        vec![
            ui::label(json!({ "id": "value", "text": label })),
            ui::button(json!({ "id": "save", "label": "Save" })),
        ],
    );
    Ok(vec![Op::PanelRender { tree }])
}
"#;
    let compiled =
        compile(&source(handler), &options("panel", EntryKind::Panel))
            .expect("compiles");

    assert!(
        compiled
            .source
            .contains("local plugin_state = state.get(STATE_KEY) or {}")
    );
    assert!(
        compiled
            .source
            .contains("local label = (plugin_state[\"label\"] or \"\")")
    );
    assert!(compiled.source.contains(
        "{ kind = \"column\", props = { id = \"root\", gap = 4 }, children = {"
    ));
    assert!(compiled.source.contains("{ kind = \"label\", props = { id = \"value\", text = label }, children = {} }"));
    assert!(compiled.source.contains("panel.render(tree)"));
    assert!(compiled.source.contains("panel.setWantsSecondTicks(true)"));
    assert!(compiled.source.contains("--!nonstrict"));
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
fn compiles_loops_and_payload_rows() {
    let handler = r#"
fn update(ctx: &Context) -> Result<Vec<Op>> {
    let rows = ctx.payload_arg(2).unwrap();
    let mut total = 0;
    let mut active = 0;
    for row in rows {
        let value = row["value"].as_i64().unwrap_or(0);
        total += value;
        if value >= 5 {
            active += 1;
        }
    }
    let status = format!("bench:{}:{}", total, active);
    Ok(vec![Op::BarText { text: status }])
}
"#;
    let compiled =
        compile(&source(handler), &options("update", EntryKind::Service))
            .expect("compiles");

    assert!(compiled.source.contains("for _, row in ipairs(rows) do"));
    assert!(
        compiled
            .source
            .contains("local value = (row[\"value\"] or 0)")
    );
    assert!(compiled.source.contains("total += value"));
    assert!(compiled.source.contains("barWidget.setText(status)"));
}

#[test]
fn rejects_unsupported_methods_with_a_position() {
    let handler = r#"
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let text = ctx.format_time("%H:%M");
    Ok(vec![Op::BarText { text }])
}
"#;
    let error =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect_err("unsupported");
    assert!(error.message.contains("format_time"));
    assert!(error.line > 0);
}

#[test]
fn rejects_a_missing_handler() {
    let error = compile(
        &source("fn other(_ctx: &Context) -> Result<Vec<Op>> { Ok(vec![]) }"),
        &options("widget", EntryKind::Widget),
    )
    .expect_err("missing");
    assert!(error.message.contains("no function named `widget`"));
}

#[test]
fn keeps_source_indentation_and_escapes() {
    let handler = r#"
fn widget(_ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![
        Op::BarText { text: "quote \" and \\ slash".to_owned() },
        Op::log("line\nbreak"),
    ])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(
        compiled
            .source
            .contains(r#"barWidget.setText("quote \" and \\ slash")"#)
    );
    assert!(compiled.source.contains(r#"noctalia.log("line\nbreak")"#));
}

#[test]
fn compiles_the_whole_op_and_host_surface() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    let flag = ctx.config_bool("flag", true);
    let now = ctx.now_ms;
    let entry = ctx.entry;
    let count = ctx.state_value::<i64>("count").unwrap_or(0);
    let name = ctx.state_value::<String>("name").unwrap_or("x");
    let setting = ctx.setting("shell.font");
    let color = ctx.color("primary");
    let dark = ctx.dark_mode();
    let vertical = ctx.bar_vertical();
    let output = ctx.bar_output_name().unwrap_or_default();
    let label = ctx.tr("greet.label", &[("name", name.as_str())]);
    let clip = ctx.clipboard();
    let stats = ctx.stats();
    let media = ctx.media();
    let calendar = ctx.calendar();
    let night = ctx.nightlight();
    let comp = ctx.compositor();
    let bar = ctx.bar();
    let dir = ctx.plugin_dir();
    let data = ctx.plugin_data_dir().unwrap_or_default();
    let status = ctx.global_state("remote");
    let src = ctx.render_source();
    let load = ctx.is_load_event();
    let fmt = ctx.time_format().unwrap_or_default();
    let date = ctx.date_format().unwrap_or_default();
    let clock = ctx.os_clock();
    let time = ctx.os_time();
    let rnd = ctx.math_random();
    let text = ctx.payload_str("text").unwrap_or_default();
    let whole = ctx.payload;
    let data_dir = ctx.data_dir;

    ops.push(Op::Notify { title: label, body: text, options: None });
    ops.push(Op::NotifyError { title: fmt, body: date, options: None });
    ops.push(Op::StateSet { key: "k", value: json!({ "count": count }) });
    ops.push(Op::TogglePanel { id: entry });
    ops.push(Op::OpenSettings);
    ops.push(Op::BarText { text: String::new() });
    ops.push(Op::BarColor { role: "primary".to_owned() });
    ops.push(Op::BarVisible { visible: flag });
    ops.push(Op::BarTooltip { value: json!([1, 2, 3]) });
    ops.push(Op::BarImage { path: dir, preserve: true, width: 26, height: 26 });
    ops.push(Op::BarRender { tree: ui::row(json!({ "id": "r" }), vec![]) });
    ops.push(Op::DesktopWidgetRender { tree: ui::scroll(json!({}), vec![ui::input(json!({})), ui::progress(json!({}))]) });
    ops.push(Op::LayerShow { id: "layer".to_owned() });
    ops.push(Op::LayerHide { id: "layer".to_owned() });
    ops.push(Op::LayerRender { tree: ui::column(json!({}), vec![]) });
    ops.push(Op::LayerConfigure { id: "layer".to_owned(), props: json!({ "a": true }) });
    ops.push(Op::LauncherResults { text: output, rows: vec![json!({ "id": 1 })] });
    ops.push(Op::RemoveFile { path: data });
    ops.push(Op::HostCall { root: "noctalia".to_owned(), path: vec!["systemStats".to_owned()], args: vec![] });
    ops.push(Op::log(format!("{} {} {} {} {} {} {} {}", now, dark, vertical, clock, time, rnd, load, src)));
    ops.push(Op::state("k", json!({ "clip": clip, "stats": stats, "media": media, "calendar": calendar, "night": night, "comp": comp, "bar": bar, "status": status, "whole": whole, "data": data_dir, "setting": setting, "color": color })));
    Ok(ops)
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Widget))
            .expect("compiles");

    for needle in [
        "bool_or(noctalia.getConfig(\"flag\"), true)",
        "noctalia.nowMs()",
        "local entry = ENTRY",
        "state[\"count\"]",
        "noctalia.getSetting(\"shell.font\")",
        "noctalia.getColor(\"primary\")",
        "noctalia.isDarkMode()",
        "barWidget.isVertical()",
        "barWidget.outputName()",
        "noctalia.tr(\"greet.label\", { name = name })",
        "noctalia.clipboardText()",
        "noctalia.systemStats()",
        "noctalia.mediaPlayer()",
        "noctalia.calendarEvents()",
        "noctalia.nightlight.state",
        "noctalia.compositor.name",
        "noctalia.pluginDir()",
        "noctalia.pluginDataDir()",
        "state.get(\"remote\")",
        "noctalia.getSetting(\"shell.time_format\")",
        "noctalia.getSetting(\"shell.date_format\")",
        "os.clock()",
        "os.time()",
        "math.random()",
        "payload[\"text\"]",
        "noctalia.notify(",
        "noctalia.notifyError(",
        "state.set(",
        "noctalia.togglePanel(entry)",
        "noctalia.openSettings()",
        "barWidget.setColor(",
        "barWidget.setVisible(flag)",
        "barWidget.setTooltip(",
        "barWidget.setImage(",
        "barWidget.render(",
        "desktopWidget.render(",
        "noctalia.layer.show(",
        "noctalia.layer.hide(",
        "layer.render(",
        "noctalia.layer.configure(",
        "launcher.setResults(",
        "noctalia.removeFile(",
        "host_call(\"noctalia\", { \"systemStats\" }, {})",
    ] {
        assert!(
            compiled.source.contains(needle),
            "missing {needle}\n{}",
            compiled.source
        );
    }
}

#[test]
fn compiles_ranges_remainders_and_unwrap() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let rows = ctx.payload_arg(0).unwrap();
    let mut total = 0;
    for index in 0..3 {
        total += index % 2;
    }
    for index in 0..=3 {
        total += rows.get(index).as_i64().unwrap_or(0);
    }
    let text = format!("{total:?}");
    Ok(vec![Op::log(text)])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("format spec is rejected");
    assert!(error.message.contains("format specifiers"));

    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let rows = ctx.payload_arg(0).unwrap();
    let mut total = 0;
    for index in 0..3 {
        total += index % 2;
    }
    for index in 0..=3 {
        total += rows.get(index).as_i64().unwrap_or(0);
    }
    Ok(vec![Op::log(format!("done {}", total))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("for index = 0, (3) - 1 do"));
    assert!(compiled.source.contains("for index = 0, 3 do"));
    assert!(compiled.source.contains("irem(index, 2)"));
    assert!(compiled.source.contains("local function irem(a, b)"));
    assert!(compiled.source.contains("unwrap(payload.args[(0) + 1])"));
    assert!(compiled.source.contains("local function unwrap(value)"));
}

#[test]
fn compiles_expressions_and_rejects_untranslatable_ones() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let trimmed = "  x  ".trim().to_uppercase();
    let lowered = trimmed.to_lowercase();
    let hit = lowered.contains("x");
    let length = lowered.len();
    let empty = lowered.is_empty();
    let first = lowered.as_str();
    let option = ctx.state_value::<String>("missing");
    let present = option.is_some();
    let absent = option.is_none();
    let cloned = option.clone();
    let cast = length as f64;
    let chosen = if hit { "yes" } else { "no" };
    Ok(vec![Op::log(format!("{} {} {} {} {} {} {} {}", first, present, absent, cloned.unwrap_or_default(), cast, empty, chosen, ctx.state_value::<String>("x").unwrap()))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("string.trim(\"  x  \")"));
    assert!(compiled.source.contains("string.upper("));
    assert!(compiled.source.contains("string.lower("));
    assert!(compiled.source.contains("string.find("));
    assert!(compiled.source.contains("#lowered"));
    assert!(compiled.source.contains("local __t1"));
    assert!(compiled.source.contains("if hit then"));
}

#[test]
fn rejects_constructs_outside_the_subset() {
    for handler in [
        "fn handler(ctx: &Context) -> Result<Vec<Op>> { let x = ctx.fuzzy_score(\"a\", \"b\"); Ok(vec![]) }",
        "fn handler(ctx: &Context) -> Result<Vec<Op>> { if let Some(x) = ctx.state_value::<String>(\"x\") { let _ = x; } Ok(vec![]) }",
        "fn handler(ctx: &Context) -> Result<Vec<Op>> { match ctx.event.as_str() { other => { let _ = other; Ok(Vec::new()) } } }",
        "fn handler(ctx: &Context) -> Result<Vec<Op>> { while ctx.now_ms > 0 { } Ok(vec![]) }",
        "fn handler(ctx: &Context) -> Result<Vec<Op>> { let x = std::process::id(); Ok(vec![]) }",
        "fn handler(ctx: &Context) -> Result<Vec<Op>> { let x = ctx.format_time(\"%H\"); Ok(vec![]) }",
        "fn handler(ctx: &Context) -> Result<Vec<Op>> { let x = ctx.command_exists(\"ls\"); Ok(vec![]) }",
    ] {
        assert!(
            compile(&source(handler), &options("handler", EntryKind::Widget))
                .is_err(),
            "should reject: {handler}"
        );
    }
}

#[test]
fn rejects_bad_signatures_and_duplicates() {
    for handler in [
        "fn handler() -> Result<Vec<Op>> { Ok(vec![]) }",
        "fn handler(a: &Context, b: &Context) -> Result<Vec<Op>> { let _ = (a, b); Ok(vec![]) }",
        "fn handler(ctx: &str) -> Result<Vec<Op>> { let _ = ctx; Ok(vec![]) }",
    ] {
        assert!(
            compile(&source(handler), &options("handler", EntryKind::Widget))
                .is_err(),
            "should reject: {handler}"
        );
    }
    let duplicate = "fn handler(_ctx: &Context) -> Result<Vec<Op>> { Ok(vec![]) }\nmod inner { fn handler(_ctx: &Context) -> Result<Vec<Op>> { Ok(vec![]) } }";
    let error =
        compile(&source(duplicate), &options("handler", EntryKind::Widget))
            .expect_err("duplicate");
    assert!(error.message.contains("more than once"));
}

#[test]
fn resolves_string_and_number_constants() {
    let handler = r#"
pub const STATE_KEY: &str = "example.state";
const RETRY_MS: u64 = 5000;

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let _ = ctx.now_ms;
    Ok(vec![
        Op::state(STATE_KEY, json!({ "retry": RETRY_MS })),
        Op::set_interval(RETRY_MS),
    ])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled
            .source
            .contains("state.set(\"example.state\", { retry = 5000 })")
    );
    assert!(compiled.source.contains("noctalia.setUpdateInterval(5000)"));
}

#[test]
fn hoists_constant_payload_tables_only_when_safe() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    match ctx.event.as_str() {
        "activate" => {
            let id = ctx.payload_str("id").unwrap_or_default();
            Ok(vec![Op::log(id)])
        }
        _ => Ok(Vec::new()),
    }
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(!compiled.source.contains("PAYLOAD_"));
    assert!(compiled.source.contains("function update() end"));
    assert!(
        compiled
            .source
            .contains("function onActivate(id) body_activate(id) end")
    );
    assert!(
        compiled
            .source
            .contains("local function body_activate(arg_id)")
    );

    let unmapped = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    match ctx.event.as_str() {
        "activate" => {
            let value = ctx.payload_arg(0).unwrap_or(0);
            Ok(vec![Op::log(format!("{value}"))])
        }
        _ => Ok(Vec::new()),
    }
}
"#;
    let compiled =
        compile(&source(unmapped), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("local PAYLOAD_UPDATE"));
    assert!(compiled.source.contains("local PAYLOAD_LOAD"));
    assert!(compiled.source.contains("body_activate(payload)"));

    let escaping = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::state("payload", ctx.payload.clone())])
}
"#;
    let compiled =
        compile(&source(escaping), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(!compiled.source.contains("PAYLOAD_UPDATE"));
    assert!(compiled.source.contains(
        "function update() body_update({ source = \"update\" }) end"
    ));
}

#[test]
fn unwraps_bools_without_replacing_false() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let on = ctx.state_value::<bool>("on").unwrap_or(true);
    Ok(vec![Op::BarVisible { visible: on }])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Widget))
            .expect("compiles");
    assert!(
        compiled
            .source
            .contains("bool_or(plugin_state[\"on\"], true)"),
        "{}",
        compiled.source
    );
    assert!(
        compiled
            .source
            .contains("local function bool_or(value, default)")
    );
    assert!(!compiled.source.contains("(plugin_state[\"on\"] or true)"));
}

#[test]
fn coerces_config_and_payload_strings_like_the_context() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let count = ctx.config_str("count").unwrap_or_default();
    let text = ctx.payload_str("text").unwrap_or_default();
    Ok(vec![Op::log(format!("{count}:{text}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(!compiled.source.contains("str_value"));
    assert!(compiled.source.contains("noctalia.getConfig(\"count\")"));
    assert!(compiled.source.contains("payload[\"text\"]"));
    assert!(compiled.source.contains("local __t"));
    assert!(compiled.source.contains("type("));
}

#[test]
fn rejects_local_functions_that_shadow_op_or_ui_names() {
    for name in ["label", "state", "log", "column", "OpenSettings"] {
        let handler = format!(
            "fn {name}() {{}}\nfn handler(ctx: &Context) -> Result<Vec<Op>> {{ let _ = {name}(); Ok(vec![]) }}"
        );
        assert!(
            compile(&source(&handler), &options("handler", EntryKind::Widget))
                .is_err(),
            "should reject shadowing `{name}`"
        );
    }
}

#[test]
fn compiles_op_constructor_call_forms() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let _ = ctx.now_ms;
    Ok(vec![
        Op::host_call("noctalia", &["systemStats"], vec![json!(1)]),
        Op::notify("title", "body"),
        Op::notify_with_options("title", "body", Some(json!({ "a": 1 }))),
        Op::notify_error("title", "body"),
        Op::copy("text"),
        Op::copy_with_mime("text", "text/plain"),
        Op::open_path("/tmp/x"),
    ])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    for needle in [
        "host_call(\"noctalia\", { \"systemStats\" }, { 1 })",
        "noctalia.notify(\"title\", \"body\", nil)",
        "noctalia.notify(\"title\", \"body\", { a = 1 })",
        "noctalia.notifyError(\"title\", \"body\", nil)",
        "noctalia.copyToClipboard(\"text\")",
        "noctalia.copyToClipboard(\"text\", \"text/plain\")",
        "noctalia.openPath(\"/tmp/x\")",
        "local function host_call(root, path, args)",
    ] {
        assert!(
            compiled.source.contains(needle),
            "missing {needle}\n{}",
            compiled.source
        );
    }
}

#[test]
fn orders_mapped_payload_params_like_the_callback() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    match ctx.event.as_str() {
        "ipc" => {
            let payload = ctx.payload_str("payload").unwrap_or_default();
            let event = ctx.payload_str("event").unwrap_or_default();
            Ok(vec![Op::log(format!("{event}={payload}"))])
        }
        _ => Ok(Vec::new()),
    }
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled
            .source
            .contains("local function body_ipc(arg_event, arg_payload)")
    );
    assert!(compiled.source.contains(
        "function onIpc(event, payload) body_ipc(event, payload) end"
    ));
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
fn coerces_unmapped_payload_scalars_to_nil() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let missing = ctx.payload_str("hovered").is_none();
    Ok(vec![Op::BarVisible { visible: missing }])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("or nil) == nil)"));
}
