use crate::{EntryKind, Module, NativeMode, Options, compile};

fn options<'a>(handler: &'a str, kind: EntryKind) -> Options<'a> {
    Options {
        handler,
        entry: "widget",
        state_key: "hello.state",
        kind,
        strings: &["message", "name"],
        module: "main",
        modules: &[],
        native: NativeMode::Deny,
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
        compiled.source.len() < 3_000,
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
    assert!(compiled.source.contains("kind = \"label\""));
    assert!(compiled.source.contains("id = \"value\""));
    assert!(compiled.source.contains("text = label"));
    assert!(compiled.source.contains("children = {}"));
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

    assert!(compiled.source.contains("local row = "));
    assert!(
        compiled
            .source
            .contains("local value = (row[\"value\"] or 0)")
    );
    assert!(compiled.source.contains("total += value"));
    assert!(compiled.source.contains("barWidget.setText(status)"));
}

#[test]
fn maps_host_reads_to_lua_globals() {
    let handler = r#"
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let exists = ctx.file_exists("~/.config");
    let command = ctx.command_exists("git");
    let contents = ctx.read_file("~/file").unwrap_or_default();
    let _ = ctx.write_file("~/out", "text");
    let home = ctx.env("HOME").unwrap_or_default();
    let path = ctx.expand_path("~/.cache").to_string_lossy().to_string();
    let entries = ctx.list_dir("~/.cache");
    let _ = ctx.mkdir_all("~/.cache/plugin");
    let _ = ctx.remove_file("~/.cache/plugin/x");
    let clock = ctx.format_time("{:%H:%M}");
    let stamp = ctx.format_time_at("%Y", Some(0), Some("UTC"));
    let now = ctx.os_time();
    let _data = ctx.plugin_data_dir_result();
    let _dir = ctx.plugin_dir_result();
    let _ = entries;
    Ok(vec![Op::log(format!("{} {} {} {} {} {} {} {}", exists, command, contents, home, path, clock, stamp, now))])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("noctalia.fileExists("));
    assert!(compiled.source.contains("noctalia.commandExists("));
    assert!(compiled.source.contains("noctalia.readFile("));
    assert!(compiled.source.contains("noctalia.writeFile("));
    assert!(compiled.source.contains("noctalia.getenv("));
    assert!(compiled.source.contains("noctalia.expandPath("));
    assert!(compiled.source.contains("noctalia.listDir("));
    assert!(compiled.source.contains("noctalia.mkdirAll("));
    assert!(compiled.source.contains("noctalia.removeFile("));
    assert!(compiled.source.contains("noctalia.formatTime("));
    assert!(
        compiled
            .source
            .contains("noctalia.formatTime(\"%Y\", (0), (\"UTC\"))")
    );
    assert!(compiled.source.contains("os.time()"));
    assert!(compiled.source.contains("noctalia.pluginDataDir()"));
    assert!(compiled.source.contains("noctalia.pluginDir()"));
}

#[test]
fn supports_additional_methods_and_match_guards() {
    let handler = r#"
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let text = ctx.config_str("message").as_deref().unwrap_or_default().to_owned();
    let mut ops = Vec::new();
    match ctx.event.as_str() {
        "render" if ctx.now_ms > 0 => ops.push(Op::BarText { text }),
        _ => {}
    }
    Ok(ops)
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("barWidget.setText(text)"));
    assert!(compiled.source.contains("and"));
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
    let text = format!("{total:x}");
    Ok(vec![Op::log(text)])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("format spec is rejected");
    assert!(
        error.message.contains("format specifier"),
        "{}",
        error.message
    );

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
    let duplicate = "mod a { fn handler(_ctx: &Context) -> Result<Vec<Op>> { Ok(vec![]) } }\nmod b { fn handler(_ctx: &Context) -> Result<Vec<Op>> { Ok(vec![]) } }";
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

fn module_options<'a>(
    handler: &'a str,
    kind: EntryKind,
    modules: &'a [Module<'a>],
) -> Options<'a> {
    Options {
        handler,
        entry: "widget",
        state_key: "hello.state",
        kind,
        strings: &["message", "name"],
        module: "main",
        modules,
        native: NativeMode::Deny,
    }
}

#[test]
fn inlines_a_module_handler() {
    let main = r#"
mod widget;
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    widget::handle(ctx)
}
"#;
    let widget = r#"
use noctalia_rs::{Context, Op, Result};

pub fn handle(ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    ops.push(Op::BarText { text: ctx.config_str("message").unwrap_or_default().into_owned() });
    Ok(ops)
}
"#;
    let modules = [Module {
        path: "widget",
        source: widget,
    }];
    let compiled = compile(
        &source(main),
        &module_options("widget", EntryKind::Widget, &modules),
    )
    .expect("compiles");
    assert!(compiled.source.contains(
        "barWidget.setText((noctalia.getConfig(\"message\") or \"\"))"
    ));
    assert!(!compiled.source.contains("widget::handle"));
    assert!(compiled.source.contains("do\n"));
}

#[test]
fn inlines_a_mut_ops_helper() {
    let handler = r#"
fn ensure(ctx: &Context, ops: &mut Vec<Op>) {
    if ctx.event == "render" {
        ops.push(Op::BarText { text: "top".to_owned() });
    }
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    ensure(ctx, &mut ops);
    ops.push(Op::log("done"));
    Ok(ops)
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("do\n"));
    assert!(compiled.source.contains("barWidget.setText(\"top\")"));
    assert!(compiled.source.contains("noctalia.log(\"done\")"));
}

#[test]
fn inlines_a_value_helper() {
    let handler = r#"
fn label(ctx: &Context) -> String {
    match ctx.event.as_str() {
        "render" => format!("shown {}", ctx.now_ms),
        _ => "hidden".to_owned(),
    }
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::BarText { text: label(ctx) }])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("local __t1"));
    assert!(
        compiled
            .source
            .contains("__t1 = \"shown \" .. noctalia.nowMs()")
    );
    assert!(compiled.source.contains("barWidget.setText(__t1)"));
}

#[test]
fn resolves_crate_self_and_super_paths() {
    let main = r#"
mod common;
mod widget;
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::log(common::label(ctx))])
}
"#;
    let common = r#"
use noctalia_rs::Context;

pub fn label(ctx: &Context) -> String {
    format!("{}", util::value(ctx))
}

pub fn suffix(_ctx: &Context) -> String {
    "!".to_owned()
}
"#;
    let util = r#"
use noctalia_rs::Context;

pub fn value(ctx: &Context) -> String {
    super::suffix(ctx)
}
"#;
    let modules = [
        Module {
            path: "common",
            source: common,
        },
        Module {
            path: "common::util",
            source: util,
        },
    ];
    let compiled = compile(
        &source(main),
        &module_options("widget", EntryKind::Widget, &modules),
    )
    .expect("compiles");
    assert!(compiled.source.contains("noctalia.log("));
    assert!(compiled.source.contains("\"!\""));
}

#[test]
fn resolves_use_aliases_and_crate_paths() {
    let main = r#"
mod panel;
use crate::panel::handle as panel_handler;
"#;
    let panel = r#"
use noctalia_rs::{Context, Op, Result};

pub fn handle(ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::log(ctx.event.as_str())])
}
"#;
    let modules = [Module {
        path: "panel",
        source: panel,
    }];
    let compiled = compile(
        &source(main),
        &module_options("panel_handler", EntryKind::Panel, &modules),
    )
    .expect("compiles");
    assert!(compiled.source.contains("noctalia.log(\"render\")"));
}

#[test]
fn resolves_constants_across_modules() {
    let main = r#"
mod common;
use crate::common::STATE;

fn widget(_ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::state(STATE, json!("x"))])
}
"#;
    let common = r#"
pub const STATE: &str = "example.state";
"#;
    let modules = [Module {
        path: "common",
        source: common,
    }];
    let compiled = compile(
        &source(main),
        &module_options("widget", EntryKind::Widget, &modules),
    )
    .expect("compiles");
    assert!(compiled.source.contains("state.set(\"example.state\""));
}

#[test]
fn rejects_recursive_and_cyclic_helpers() {
    let handler = r#"
fn first(ctx: &Context, ops: &mut Vec<Op>) {
    second(ctx, ops);
}
fn second(ctx: &Context, ops: &mut Vec<Op>) {
    first(ctx, ops);
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    first(ctx, &mut ops);
    Ok(ops)
}
"#;
    let error =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect_err("cycle");
    assert!(
        error.message.contains("recursive helper"),
        "{}",
        error.message
    );
}

#[test]
fn supports_early_return_and_question_mark_in_helpers() {
    for helper in [
        "fn helper(ctx: &Context, ops: &mut Vec<Op>) { if ctx.now_ms > 0 { return; } ops.push(Op::log(\"x\")); }",
        "fn helper(ctx: &Context, ops: &mut Vec<Op>) { let value = ctx.config_str(\"message\")?; ops.push(Op::log(value)); }",
    ] {
        let handler = format!(
            "{helper}\nfn widget(ctx: &Context) -> Result<Vec<Op>> {{\n    let mut ops = Vec::new();\n    if ctx.event == \"render\" {{ helper(ctx, &mut ops); }}\n    Ok(ops)\n}}\n"
        );
        let compiled =
            compile(&source(&handler), &options("widget", EntryKind::Widget))
                .expect("compiles");
        assert!(compiled.source.contains("(function()"));
        assert!(compiled.source.contains("end)()"));
    }
}

#[test]
fn compiles_closures_in_helpers() {
    let helper = "fn helper(ctx: &Context, ops: &mut Vec<Op>) { let find = |a: i64| a; let _ = find(1); }";
    let handler = format!(
        "{helper}\nfn widget(ctx: &Context) -> Result<Vec<Op>> {{\n    let mut ops = Vec::new();\n    if ctx.event == \"render\" {{ helper(ctx, &mut ops); ops.push(Op::log(\"x\")); }}\n    Ok(ops)\n}}\n"
    );
    let compiled =
        compile(&source(&handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("function(a)"));
}

#[test]
fn rejects_mut_borrows_that_cannot_be_inlined() {
    let handler = r#"
fn helper(word: &mut String) {
    word.push('x');
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let mut word = String::new();
    helper(&mut word);
    Ok(vec![Op::log(word)])
}
"#;
    let error =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect_err("bail");
    assert!(error.message.contains("&mut"), "{}", error.message);
}

#[test]
fn rejects_unresolved_module_calls() {
    let handler = r#"
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    missing::handle(ctx)
}
"#;
    let error =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect_err("bail");
    assert!(
        error.message.contains("call to `missing::handle`"),
        "{}",
        error.message
    );
}

#[test]
fn rejects_queries_that_need_a_response_path() {
    let handler = r#"
fn widget(_ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::query("id", "noctalia", &["nowMs"], vec![], "out.json")])
}
"#;
    let error =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect_err("bail");
    assert!(error.message.contains("response path"), "{}", error.message);
}

#[test]
fn reports_positions_from_the_helper_module() {
    let main = r#"
mod widget;
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    widget::handle(ctx)
}
"#;
    let widget = r#"
use noctalia_rs::{Context, Op, Result};

pub fn handle(_ctx: &Context) -> Result<Vec<Op>> {
    let bad = std::process::id();
    let _ = bad;
    Ok(Vec::new())
}
"#;
    let modules = [Module {
        path: "widget",
        source: widget,
    }];
    let error = compile(
        &source(main),
        &module_options("widget", EntryKind::Widget, &modules),
    )
    .expect_err("bail");
    assert_eq!(
        error.render("main.rs"),
        format!("widget:{}:{}: {}", error.line, error.column, error.message)
    );
    assert!(error.line > 0);
}

#[test]
fn finds_a_handler_in_an_inline_module() {
    let handler = r#"
mod inner {
    pub fn widget(_ctx: &Context) -> Result<Vec<Op>> {
        Ok(vec![Op::log("inner")])
    }
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("noctalia.log(\"inner\")"));
}

#[test]
fn resolves_self_paths_glob_imports_and_super_chains() {
    let main = r#"
mod outer;
use crate::shared::*;
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::log(self::local(ctx)), Op::log(globbed(ctx))])
}
fn local(_ctx: &Context) -> String {
    "local".to_owned()
}
"#;
    let outer = r#"
mod inner;

pub fn label(ctx: &Context) -> String {
    super::super::local(ctx)
}
"#;
    let inner = r#"
use crate::outer::label;
use noctalia_rs::Context;

pub fn deep(ctx: &Context) -> String {
    self::suffix(ctx)
}

fn suffix(_ctx: &Context) -> String {
    label(ctx)
}
"#;
    let shared = r#"
use noctalia_rs::Context;

pub fn globbed(_ctx: &Context) -> String {
    "glob".to_owned()
}
"#;
    let modules = [
        Module {
            path: "outer",
            source: outer,
        },
        Module {
            path: "outer::inner",
            source: inner,
        },
        Module {
            path: "shared",
            source: shared,
        },
    ];
    let compiled = compile(
        &source(main),
        &module_options("widget", EntryKind::Widget, &modules),
    )
    .expect("compiles");
    assert!(compiled.source.contains("\"local\""));
    assert!(compiled.source.contains("\"glob\""));
}

#[test]
fn resolves_constants_through_self_and_super() {
    let main = r#"
mod common;
fn widget(_ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::state(common::STATE, json!("x"))])
}
"#;
    let common = r#"
pub const STATE: &str = "example.state";
pub mod nested {
    pub const KEY: &str = "nested.key";
}
"#;
    let modules = [Module {
        path: "common",
        source: common,
    }];
    let compiled = compile(
        &source(main),
        &module_options("widget", EntryKind::Widget, &modules),
    )
    .expect("compiles");
    assert!(compiled.source.contains("state.set(\"example.state\""));
}

#[test]
fn inlines_vec_ops_helpers_and_final_returns() {
    let handler = r#"
fn build(ctx: &Context) -> Vec<Op> {
    let mut ops = Vec::new();
    ops.push(Op::log(ctx.event.as_str()));
    ops
}
fn offset(ctx: &Context) -> i64 {
    return ctx.now_ms;
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let now = offset(ctx);
    Ok(build(ctx))
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("noctalia.log(\"render\")"));
}

#[test]
fn covers_more_methods_and_guarded_wildcards() {
    let handler = r#"
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let mut rows = Vec::new();
    rows.extend(vec![json!("a"), json!("b")]);
    let shown = ctx.expand_path("~/x").display().to_string();
    let mut ops = Vec::new();
    match ctx.event.as_str() {
        "render" | "click" => ops.push(Op::log(shown)),
        _ if ctx.now_ms > 0 => ops.push(Op::log("later")),
        _ => {}
    }
    let _ = rows;
    Ok(ops)
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("ipairs("));
    assert!(compiled.source.contains("elseif true then"));
    assert!(compiled.source.contains("if (noctalia.nowMs() > 0) then"));
}

#[test]
fn rejects_inline_signature_mismatches() {
    for handler in [
        r#"
fn helper(ctx: &Context) {}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    helper(ctx, ctx);
    Ok(Vec::new())
}
"#,
        r#"
fn helper(other: &Context) { let _ = other; }
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    helper(missing);
    Ok(Vec::new())
}
"#,
        r#"
fn helper(ops: &mut Vec<Op>) {}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    helper(&mut Vec::new());
    Ok(Vec::new())
}
"#,
        r#"
fn helper(ctx: &Context) -> Result<String> { Ok(String::new()) }
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    helper(ctx)
}
"#,
        r#"
fn helper(ctx: &Context) -> Vec<Op> { Vec::new() }
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    helper(ctx)
}
"#,
        r#"
fn helper(ctx: &Context) -> Result<Vec<Op>> { Ok(Vec::new()) }
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let value = helper(ctx);
    Ok(value)
}
"#,
        r#"
        r#"
fn helper(ctx: &Context) -> String { let _ = ctx; }
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let value = helper(ctx);
    Ok(vec![Op::log(value)])
}
"#,
    ] {
        assert!(
            compile(&source(handler), &options("widget", EntryKind::Widget))
                .is_err(),
            "should bail: {handler}"
        );
    }
}

#[test]
fn rejects_inline_depth_and_unsupported_ops() {
    let mut handler = String::new();
    for index in 0..10 {
        handler.push_str(&format!(
            "fn helper{index}(ctx: &Context, ops: &mut Vec<Op>) {{\n    helper{}(ctx, ops);\n}}\n",
            index + 1
        ));
    }
    handler.push_str("fn helper10(_ctx: &Context, _ops: &mut Vec<Op>) {}\n");
    handler.push_str(
        "fn widget(ctx: &Context) -> Result<Vec<Op>> {\n    let mut ops = Vec::new();\n    helper0(ctx, &mut ops);\n    Ok(ops)\n}\n",
    );
    let error =
        compile(&source(&handler), &options("widget", EntryKind::Widget))
            .expect_err("depth");
    assert!(error.message.contains("depth limit"), "{}", error.message);

    let handler = r#"
fn widget(_ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::query("id", "noctalia", &["nowMs"], vec![], "out.json")])
}
"#;
    let error =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect_err("query");
    assert!(error.message.contains("response path"), "{}", error.message);
}

#[test]
fn rejects_parse_errors_and_renders_them() {
    let error = compile(
        "fn widget(ctx: &Context) -> Result<Vec<Op>> { Ok(vec![Op::Nope]) }",
        &options("widget", EntryKind::Widget),
    )
    .expect_err("op");
    assert!(error.line > 0);
    assert!(error.to_string().contains("line "));
    assert!(error.render("main.rs").contains("main.rs:"));
    let plain = crate::Error::plain("plain");
    assert_eq!(plain.render("main.rs"), "main.rs: plain");
    let parse = compile("fn {", &options("widget", EntryKind::Widget))
        .expect_err("parse");
    assert!(parse.render("main.rs").contains("main:"));
}

#[test]
fn formats_time_at_with_optional_arguments() {
    let handler = r#"
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let short = ctx.format_time_at("%H", Some(0));
    let long = ctx.format_time_at("%H", Some(0), Some("UTC"));
    Ok(vec![Op::log(format!("{} {}", short, long))])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(
        compiled
            .source
            .contains("noctalia.formatTime(\"%H\", (0), nil)")
    );
    assert!(
        compiled
            .source
            .contains("noctalia.formatTime(\"%H\", (0), (\"UTC\"))")
    );
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
fn lowers_question_mark_on_options_and_results() {
    let handler = r#"
fn option(ctx: &Context, ops: &mut Vec<Op>) {
    let value = ctx.config_str("message")?;
    ops.push(Op::log(value));
}
fn result(ctx: &Context) -> Result<i64> { Ok(ctx.now_ms) }
fn value(ctx: &Context, ops: &mut Vec<Op>) {
    let now = result(ctx)?;
    ops.push(Op::log(format!("{}", now)));
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    option(ctx, &mut ops);
    value(ctx, &mut ops);
    Ok(ops)
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("== nil then return end"));
}

#[test]
fn compiles_once_guards_and_once_locks() {
    let handler = r#"
use std::sync::{Once, OnceLock};
static VALUE: OnceLock<String> = OnceLock::new();
static ONCE: Once = Once::new();
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    let value = VALUE.get_or_init(|| "ready".to_owned());
    ONCE.call_once(|| ops.push(Op::log("once")));
    VALUE.set("later".to_owned());
    ops.push(Op::log(value));
    ops.push(Op::log(format!("{}", ctx.event.as_str())));
    Ok(ops)
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("local __static_main__VALUE"));
    assert!(
        compiled
            .source
            .contains("if __static_main__VALUE == nil then")
    );
    assert!(compiled.source.contains("if not __static_main__ONCE then"));
    assert!(compiled.source.contains("__static_main__ONCE = true"));
}

#[test]
fn compiles_option_combinators_with_closures() {
    let handler = r#"
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let value = ctx.config_str("message").map(|text| text.to_uppercase());
    let bool_value = ctx.config_str("message").and_then(Value::as_bool);
    let kept = ctx.config_str("message").filter(|text| !text.is_empty());
    let fallback = ctx.config_str("message").unwrap_or_else(|| "none".to_owned());
    Ok(vec![Op::log(format!("{} {} {} {}", value.unwrap_or_default(), bool_value.unwrap_or(false), kept.unwrap_or_default(), fallback))])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("~= nil then"));
    assert!(compiled.source.contains("function(__value)"));
    assert!(!compiled.source.contains("not text.is_empty"));
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
    assert!(compiled.source.contains("value ~= nil"));
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
fn covers_static_containers_and_atomic_operations() {
    let handler = r#"
use std::sync::{Once, OnceLock, Mutex, RwLock, atomic::{AtomicBool, AtomicU64, Ordering}};
static FLAG: Once = Once::new();
static CELL: OnceLock<String> = OnceLock::new();
static LOCK: Mutex<i64> = Mutex::new(1);
static RW: RwLock<i64> = RwLock::new(2);
static BOOL: AtomicBool = AtomicBool::new(false);
static COUNT: AtomicU64 = AtomicU64::new(0);
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    FLAG.call_once(|| ops.push(Op::log("once")));
    let value = CELL.get_or_init(|| "init".to_owned());
    CELL.set("set".to_owned());
    let _ = CELL.take();
    let _ = LOCK.lock().unwrap();
    let _ = RW.read().unwrap();
    let _ = RW.write().unwrap();
    BOOL.store(true, Ordering::SeqCst);
    let _ = BOOL.load(Ordering::SeqCst);
    let _ = COUNT.swap(1, Ordering::SeqCst);
    let _ = COUNT.fetch_add(1, Ordering::SeqCst);
    let _ = COUNT.fetch_sub(1, Ordering::SeqCst);
    ops.push(Op::log(value));
    Ok(ops)
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("__static_main__CELL"));
}

#[test]
fn covers_impl_methods_and_struct_fields() {
    let handler = r#"
struct State { name: String, count: i64 }
impl State {
    fn new(ctx: &Context) -> State { State { name: ctx.config_str("message").unwrap_or_default().into_owned(), count: 0 } }
    fn bump(&mut self, amount: i64) { self.count += amount; }
    fn label(&self) -> String { self.name.clone() }
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let mut state = State::new(ctx);
    state.bump(2);
    let label = state.label();
    state.name = "changed".to_owned();
    Ok(vec![Op::log(label)])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("self.count += amount"));
    assert!(compiled.source.contains("state.name ="));
}

#[test]
fn covers_all_option_combinators() {
    let handler = r#"
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let base = ctx.config_str("message");
    let mapped = base.clone().map(|value| value.to_uppercase());
    let chained = base.clone().and_then(Value::as_str);
    let filtered = base.clone().filter(|value| !value.is_empty());
    let some = base.clone().is_some_and(|value| value == "x");
    let either = base.clone().map_or("default".to_owned(), |value| value);
    let or_else = base.clone().map_or_else(|| "fallback".to_owned(), |value| value);
    let fallback = base.clone().unwrap_or_else(|| "none".to_owned());
    let alt = base.clone().or_else(|| Some("alt".to_owned()));
    let result = base.clone().ok_or("missing");
    let result2 = base.clone().ok_or_else(|| "missing");
    Ok(vec![Op::log(format!("{} {} {} {} {} {} {} {} {} {}", mapped.unwrap_or(""), chained.unwrap_or(""), filtered.unwrap_or(""), some, either, or_else, fallback, alt.unwrap_or(""), result.unwrap_or(""), result2.unwrap_or("")))])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("function(__value)"));
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
fn covers_expression_subset_additions() {
    let handler = r#"
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let (first, _, third) = ("a", "b", "c");
    let map = std::collections::HashMap::new();
    let set = std::collections::BTreeSet::new();
    let null = Value::Null;
    let boolean = Value::Bool(true);
    let number = Value::Number(1.0);
    let tree = ui::node("box", json!({ "id": "root" }), vec![]);
    let key = "message";
    let from_config = ctx.config.get(key);
    let indexed = ctx.config["message"];
    let number_value = from_config.and_then(Value::as_f64).unwrap_or(0.0);
    if let Some(text) = ctx.config_str("message") {
        let wrapped = &text;
        let value = *wrapped;
        Ok(vec![Op::log(format!("{}{}{}{}{}{}{}{}", first, third, map.len(), set.len(), number, boolean, tree.kind, number_value + value.len() as f64))])
    } else {
        let Some(fallback) = indexed.and_then(Value::as_str) else {
            return Ok(vec![Op::log("fallback")]);
        };
        Ok(vec![Op::log(fallback)])
    }
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("noctalia.getConfig"));
}

#[test]
fn covers_static_initializers_and_remaining_methods() {
    let handler = r#"
use std::cell::{Cell, RefCell};
use std::sync::{LazyLock, Mutex, atomic::{AtomicBool, Ordering}};
struct Config { enabled: bool, name: String }
static CONFIG: Mutex<Config> = Mutex::new(Config { enabled: true, name: "ready" });
static LAZY: LazyLock<String> = LazyLock::new(|| "lazy".to_owned());
static CELL: Cell<i64> = Cell::new(1);
static REF: RefCell<String> = RefCell::new("shared".to_owned());
static FLAG: AtomicBool = AtomicBool::new(true);
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let value = LAZY.deref();
    let cell = CELL.get();
    let shared = REF.borrow();
    let enabled = FLAG.load(Ordering::SeqCst);
    let config = CONFIG.lock().unwrap();
    let mut optional: Option<String> = None;
    let inserted = optional.get_or_insert("inserted".to_owned());
    let parsed = ctx.config_str("message").and_then(|text| text.parse::<f64>().ok());
    let expected = ctx.config_str("message").expect("missing");
    let map = std::collections::BTreeMap::new();
    let set = std::collections::HashSet::new();
    let list = std::collections::HashMap::new();
    Ok(vec![Op::log(format!("{}{}{}{}{}{}{}{}{}{}{}", value, cell, shared, enabled, config.enabled, inserted, parsed.unwrap_or(0.0), expected, map.len(), set.len(), list.len()))])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("local __static_main__CONFIG"));
}

#[test]
fn covers_local_statics_and_guard_helpers() {
    let handler = r#"
use std::sync::{Once, OnceLock};
fn guard(ctx: &Context, ops: &mut Vec<Op>) {
    static LOCAL_ONCE: Once = Once::new();
    static LOCAL_VALUE: OnceLock<String> = OnceLock::new();
    LOCAL_ONCE.call_once(|| ops.push(Op::log("local")));
    let value = LOCAL_VALUE.get_or_init(|| ctx.tr("key", &[]));
    ops.push(Op::log(value));
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    guard(ctx, &mut ops);
    Ok(ops)
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("__local_"));
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
fn covers_additional_bail_and_expression_branches() {
    for handler in [
        "fn widget(ctx: &Context) -> Result<Vec<Op>> { let value = ctx.config_str(\"message\").map_err(|e| e); let _ = value; Ok(vec![]) }",
        "fn widget(ctx: &Context) -> Result<Vec<Op>> { let value = ctx.config_str(\"message\").map(|a: i64, b: i64| a + b); let _ = value; Ok(vec![]) }",
        "fn widget(ctx: &Context) -> Result<Vec<Op>> { static X: std::sync::OnceLock<String> = std::sync::OnceLock::new(); X.compare_exchange(1, 2); Ok(vec![]) }",
        "fn widget(ctx: &Context) -> Result<Vec<Op>> { let f = |(a, b): (i64, i64)| a + b; let _ = f; Ok(vec![]) }",
    ] {
        let error =
            compile(&source(handler), &options("widget", EntryKind::Widget))
                .expect_err("bail");
        assert!(!error.message.is_empty());
    }
    let handler = r#"
struct State { count: i64 }
impl State {
    fn new() -> State { State { count: 1 } }
    fn empty(&self) -> bool { self.count == 0 }
}
fn widget(_ctx: &Context) -> Result<Vec<Op>> {
    let state = State::new();
    let empty = state.empty();
    let (a, b) = (1, 2);
    if let value = Some(a) {
        let Some(text) = Some("x") else { return Ok(vec![]); };
        return Ok(vec![Op::log(format!("{}{}{}{}", empty, a, b, text))]);
    }
    Ok(vec![])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("local a, b ="));
}

#[test]
fn emits_closures_as_lua_functions() {
    let handler = r#"
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let double = |value: i64| value * 2;
    let add = |left: i64, right: i64| { left + right };
    let first = double(2);
    let second = add(1, 2);
    let ignored = |_value: i64| 0;
    let third = ignored(1);
    Ok(vec![Op::log(ctx.tr("key", &[("first", json!(first)), ("second", json!(second)), ("third", json!(third))]))])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("function(value)"));
    assert!(compiled.source.contains("function(left, right)"));
    assert!(compiled.source.contains("double(2)"));
}

#[test]
fn covers_remaining_expression_and_error_paths() {
    let handler = r#"
use std::sync::OnceLock;
struct State { count: i64 }
impl State {
    fn new(value: i64) -> State { State { count: value } }
    fn bump(&mut self, amount: i64, extra: i64) { self.count += amount + extra; }
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let state = State::new(1);
    let mut mutable = state;
    mut_helper(&mut mutable, ctx);
    let cell: OnceLock<String> = OnceLock::new();
    let _ = cell.get();
    let value = ctx.config.get("message");
    let indexed = ctx.config["name"];
    let object = value.and_then(Value::as_object);
    let number = value.and_then(Value::as_f64);
    let flag = matches!(value, Value::Null | Some(Value::Bool(false)));
    let tuple = match value { Some(text) => (text.to_string(), 1), None => ("none".to_owned(), 0) };
    let tree = ui::node("box", json!({}), vec![]);
    Ok(vec![Op::log(format!("{}{}{}{}{}{}{}", mutable.count, object.is_some(), number.is_some(), flag, tuple.0, tuple.1, tree.kind))])
}
fn mut_helper(state: &mut State, _ctx: &Context) {
    state.bump(1, 2);
}
"#;
    for handler in [
        "struct State { count: i64 } impl State { fn new() -> State { State { count: 1 } } } fn widget(_ctx: &Context) -> Result<Vec<Op>> { let s = State::new(2); let _ = s; Ok(vec![]) }",
        "struct State { count: i64 } impl State { fn bump(&self, a: i64, b: i64) { let _ = (a, b); } } fn widget(_ctx: &Context) -> Result<Vec<Op>> { let s = State { count: 1 }; s.bump(1); Ok(vec![]) }",
    ] {
        let error =
            compile(&source(handler), &options("widget", EntryKind::Widget))
                .expect_err("bail");
        assert!(error.line > 0);
    }

    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("noctalia.getConfig"));
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
fn covers_arithmetic_host_reads_and_pattern_edges() {
    let handler = r#"
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let a = 10 - 3;
    let b = 10 / 3;
    let c = 10.0 / 3.0;
    let d = 10 % 3;
    let e = 10.0 % 3.0;
    let now = ctx.host("now_ms").and_then(Value::as_i64).unwrap_or(0);
    let dark = ctx.host("dark").and_then(Value::as_bool).unwrap_or(false);
    let outputs = ctx.host("outputs").unwrap_or(json!([]));
    let focused = ctx.host("focused_output_name").and_then(Value::as_str).unwrap_or("");
    let key = "message";
    let from_key = ctx.config.get(key);
    let pair = match from_key { (first, second) => first, _ => "none" };
    let closure = |text: &str| { let upper = text.to_uppercase(); upper };
    let upper = closure("x");
    Ok(vec![Op::log(format!("{}{}{}{}{}{}{}{}{}{}", a, b, c, d, e, now, dark, outputs, focused, pair.len() + upper.len()))])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("noctalia.nowMs()"));
}

#[test]
fn unconditional_handler_emits_every_callback() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let _ = ctx.now_ms;
    Ok(vec![Op::log("tick")])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Widget))
            .expect("compiles");
    for callback in crate::CALLBACKS {
        let expected = format!("function {}(", callback.function);
        assert!(
            compiled.source.contains(&expected),
            "missing {}",
            callback.function
        );
    }
    assert!(compiled.events.is_empty());
    assert!(compiled.source.contains("noctalia.log(\"tick\")"));
}

#[test]
fn matches_macro_records_event_callbacks() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if matches!(ctx.event.as_str(), "click") {
        Ok(vec![Op::log("click")])
    } else {
        Ok(Vec::new())
    }
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("function onClick()"));
    assert_eq!(compiled.events, vec!["click"]);
}

#[test]
fn lazy_fallback_closures_stay_inside_the_nil_branch() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let value = ctx.state_value::<String>("value").map_or_else(
        || "fallback".to_owned(),
        |text| text,
    );
    let _ = ctx.state_value::<String>("other").ok_or_else(|| "missing");
    Ok(vec![Op::log(value)])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled
            .source
            .contains("local __t1 = plugin_state[\"value\"]")
    );
    assert!(compiled.source.contains("if __t1 == nil then"));
    assert!(compiled.source.contains("if __t3 == nil then"));
    assert!(
        !compiled
            .source
            .contains("local __t4 = error(tostring(\"missing\"))")
    );
}

#[test]
fn negated_event_keeps_the_other_callbacks() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event != "click" {
        Ok(vec![Op::log("tick")])
    } else {
        Ok(Vec::new())
    }
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("function onClick()"));
    assert!(compiled.source.contains("function onHover(hovered)"));
    assert!(compiled.source.contains("function update()"));
    assert!(compiled.source.contains("noctalia.log(\"tick\")"));
}

#[test]
fn unconditional_handler_reads_the_runtime_event() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::log(ctx.event.to_owned())])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("local function body_events("));
    assert!(compiled.source.contains("noctalia.log(event)"));
    assert!(
        compiled
            .source
            .contains("function onClick() body_events(\"click\") end")
    );
    assert!(compiled.source.contains("function update() body_"));
}

#[test]
fn fallback_body_serves_events_without_a_positive_branch() {
    for condition in ["ctx.event == \"click\"", "ctx.event != \"click\""] {
        let handler = format!(
            "fn handler(ctx: &Context) -> Result<Vec<Op>> {{\n    if {condition} {{\n        Ok(vec![Op::log(\"A\")])\n    }} else {{\n        Ok(vec![Op::log(\"B\")])\n    }}\n}}\n"
        );
        let compiled =
            compile(&source(&handler), &options("handler", EntryKind::Widget))
                .expect("compiles");
        for callback in crate::CALLBACKS {
            let expected = format!("function {}(", callback.function);
            assert!(
                compiled.source.contains(&expected),
                "{condition} missing {}",
                callback.function
            );
        }
        assert!(compiled.source.contains("local function body_events("));
        assert!(compiled.source.contains("noctalia.log(\"B\")"));
    }
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
fn compiles_iterator_chains_and_split_cursors() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let values = [1, 2, 3];
    let doubled = values.iter().map(|value| value * 2).collect::<Vec<i64>>();
    let kept = values.iter().filter(|value| *value > 1).collect::<Vec<i64>>();
    let words = "a  b".split_whitespace().collect::<Vec<&str>>();
    let mut parts = "a.b".split('.');
    let first = parts.next().unwrap_or_default();
    Ok(vec![Op::log(format!("{}{}{}{}{}", doubled.len(), kept.len(), words.len(), first, ctx.event))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("string.split("));
    assert!(compiled.source.contains("__next = 1"));
    assert!(compiled.source.contains("#__t"));
    assert!(compiled.source.contains("if __t"));
}

#[test]
fn compiles_numeric_string_and_path_methods() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = 1.25_f64;
    let rounded = value.floor();
    let fraction = value.fract();
    let finite = value.is_finite();
    let joined = "/tmp".join("file");
    let replaced = "a,b".replace(',', "-");
    let trimmed = "'hi'".trim_matches('\'');
    let encoded = serde_json::Value::String("x".to_owned()).to_string();
    Ok(vec![Op::log(format!("{}{}{}{}{}{}{}", rounded, fraction, finite, joined, replaced, trimmed, encoded))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("math.floor(value)"));
    assert!(compiled.source.contains("string.sub(__t"));
    assert!(compiled.source.contains("replace_all("));
    assert!(compiled.source.contains("string.gsub("));
    assert!(compiled.source.contains("json.encode("));
}

#[test]
fn compiles_struct_defaults_and_tuple_loops() {
    let handler = r#"
#[derive(Default)]
struct State { count: i64, label: String }
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let state = State::default();
    let pairs = [("a", 1), ("b", 2)];
    let mut total = 0;
    for (name, value) in pairs {
        total += value;
    }
    let indexed = [(0, "a"), (1, "b")];
    for (index, value) in indexed.iter().enumerate() {
        total += index;
    }
    Ok(vec![Op::log(format!("{}{}{}{}", state.count, state.label, total, ctx.event))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("count = 0, label = \"\""));
    assert!(compiled.source.contains("local name = __t"));
    assert!(compiled.source.contains("local index = (__t"));
}

#[test]
fn compiles_while_loops_blocks_and_array_repeat() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let flags = [false; 3];
    let total = {
        let mut value = 0;
        while value < 10 {
            value += 1;
            if value == 3 { break; }
        }
        value
    };
    Ok(vec![Op::log(format!("{}{}", flags.len(), total))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("{ false, false, false }"));
    assert!(compiled.source.contains("while (value < 10) do"));
    assert!(compiled.source.contains("break"));
}

#[test]
fn compiles_host_result_pairs() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let (text, error) = ctx.read_file_result("/tmp/x");
    let (font, font_error) = ctx.load_font_result("font.ttf");
    Ok(vec![Op::log(format!("{}{}{}{}", text.unwrap_or_default(), error.is_some(), font.is_some(), font_error.is_some()))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("local text, "));
    assert!(compiled.source.contains("noctalia.readFile(\"/tmp/x\")"));
    assert!(compiled.source.contains("noctalia.loadFont(\"font.ttf\")"));
}

#[test]
fn bails_on_host_start_methods_that_report_acceptance() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if !ctx.run_async_start(json!([]), None, None) {
        return Ok(Vec::new());
    }
    Ok(Vec::new())
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(error.message.contains("must be discarded"));
    assert!(error.line > 0 && error.column > 0);
}

#[test]
fn compiles_result_combinators_and_defaults() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let parsed = serde_json::from_str::<Value>("{}").map_err(|_| "bad")?;
    let repaired = serde_json::from_str::<Value>("x").unwrap_or_else(|_| Value::Null);
    let home = std::env::var("HOME").unwrap_or_default();
    let fallback = parsed.clone().unwrap_or(Value::Null);
    let optional = parsed.clone().ok();
    let expected = parsed.clone().expect("data");
    let value = parsed.unwrap();
    Ok(vec![Op::log(format!("{} {} {} {} {} {}", home, fallback, optional.is_some(), expected, value, ctx.event))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("json.decode(\"{}\")"));
    assert!(compiled.source.contains("error(tostring("));
    assert!(compiled.source.contains("noctalia.getenv(\"HOME\")"));
    assert!(compiled.source.contains("if __t"));
}

#[test]
fn compiles_remaining_numeric_and_string_methods() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let number = 1.5_f64;
    let bounded = number.clamp(0.0, 1.0);
    let largest = number.max(2.0);
    let smallest = number.min(2.0);
    let positive = number.rem_euclid(2.0);
    let suffix = "prefix:value".strip_prefix("prefix:").unwrap_or_default();
    let upper = "abc".to_ascii_uppercase();
    let lower = "ABC".to_ascii_lowercase();
    let starts = "abc".starts_with("a");
    let ends = "abc".ends_with("c");
    Ok(vec![Op::log(format!("{}{}{}{}{}{}{}{}{}", bounded, largest, smallest, positive, suffix, upper, lower, starts, ends))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("math.clamp("));
    assert!(compiled.source.contains("math.max("));
    assert!(compiled.source.contains("math.min("));
    assert!(compiled.source.contains("string.sub("));
    assert!(compiled.source.contains("string.upper("));
    assert!(compiled.source.contains("string.lower("));
}

#[test]
fn compiles_iterator_source_variants() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let parts = "a,b".split(',').collect::<Vec<&str>>();
    let letters = "ab".chars().collect::<Vec<char>>();
    let lines = "a\nb".lines().collect::<Vec<&str>>();
    let values = [1, 2].into_iter().filter_map(|value| Some(value)).collect::<Vec<i64>>();
    let mut table = std::collections::HashMap::new();
    let keys = table.keys().collect::<Vec<&str>>();
    let entries = table.values().collect::<Vec<&i64>>();
    Ok(vec![Op::log(format!("{}{}{}{}{}{}", parts.len(), letters.len(), lines.len(), values.len(), keys.len(), entries.len()))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("string.split("));
    assert!(compiled.source.contains("for _, __t"));
    assert!(compiled.source.contains("in pairs("));
    assert!(compiled.source.contains(".__next or 1, #"));
}

#[test]
fn compiles_remaining_host_results_and_time_helpers() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let (entries, list_error) = ctx.list_dir_result("/tmp");
    let (info, info_error) = ctx.file_info_result("/tmp/x");
    let (made, mkdir_error) = ctx.mkdir_all_result("/tmp/y");
    let (removed, remove_error) = ctx.remove_file_result("/tmp/z");
    ctx.math_randomseed(7);
    let random = ctx.math_random_range(1, 6);
    let localized = ctx.trp("key", 3, &[("name", json!("x"))]);
    let load = ctx.is_load_event();
    Ok(vec![Op::log(format!("{}", entries.len() + info.len() + made.len() + removed.len() + list_error.is_some() as usize + info_error.is_some() as usize + mkdir_error.is_some() as usize + remove_error.is_some() as usize + random as usize + localized.len() + load as usize))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("noctalia.listDir(\"/tmp\")"));
    assert!(compiled.source.contains("noctalia.fileInfo("));
    assert!(compiled.source.contains("noctalia.mkdirAll("));
    assert!(compiled.source.contains("noctalia.removeFile("));
    assert!(compiled.source.contains("math.randomseed(7)"));
    assert!(compiled.source.contains("math.random(1, 6)"));
    assert!(
        compiled
            .source
            .contains("noctalia.trp(\"key\", 3, { name = ")
    );
}

#[test]
fn compiles_remaining_host_paths() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let dir = ctx.host("pluginDir").unwrap_or_default();
    let data = ctx.host("pluginDataDir").unwrap_or_default();
    let clip = ctx.host("clipboard").unwrap_or_default();
    let calendar = ctx.host("calendar").unwrap_or_default();
    let media = ctx.host("media").unwrap_or_default();
    let stats = ctx.host("stats").unwrap_or_default();
    let night = ctx.host("nightlight.state").unwrap_or_default();
    let compositor = ctx.host("compositor.name").unwrap_or_default();
    let vertical = ctx.host("bar.vertical").unwrap_or_default();
    let output = ctx.host("bar.output_name").unwrap_or_default();
    Ok(vec![Op::log(format!("{}{}{}{}{}{}{}{}{}{}", dir, data, clip, calendar, media, stats, night, compositor, vertical, output))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("noctalia.pluginDir()"));
    assert!(compiled.source.contains("noctalia.clipboardText()"));
    assert!(compiled.source.contains("barWidget.isVertical()"));
    assert!(compiled.source.contains("noctalia.compositor.name"));
    let error = compile(
        &source("fn handler(ctx: &Context) -> Result<Vec<Op>> { let _ = ctx.host(\"missing\"); Ok(vec![]) }"),
        &options("handler", EntryKind::Service),
    )
    .expect_err("unknown host path");
    assert!(error.message.contains("host path `missing`"));
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
fn compiles_host_call_struct_and_watch_op() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![
        Op::HostCall { root: "noctalia".to_owned(), path: vec!["run".to_owned()], args: vec![json!(1)] },
        Op::state_watch("key", "callback"),
    ])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled
            .source
            .contains("host_call(\"noctalia\", { \"run\" }, { 1 })")
    );
    assert!(
        compiled.source.contains(
            "state.watch(\"key\", { __host_callback = \"callback\" })"
        )
    );
}

#[test]
fn scans_helper_expressions_and_inlines_a_manual_default() {
    let handler = r#"
struct State { count: i64 }
impl State {
    fn default() -> State { State { count: 1 } }
    fn scan(&self, ops: &mut Vec<Op>) {
        let value = -1;
        let wrapped = (value);
        let reference = &wrapped;
        let mut total = 0;
        total += *reference;
        let cast = total as f64;
        let array = [1, 2];
        let tuple = (1, 2);
        let nested = State { count: 2 };
        let _ = array[0];
        let _ = nested.count;
        let _ = tuple.0;
        let _ = wrapped.abs();
        let _ = format!("{}", cast);
        ops.push(Op::log("scanned"));
    }
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let state = State::default();
    let mut ops = Vec::new();
    state.scan(&mut ops);
    let _ = ctx.now_ms;
    Ok(ops)
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("count = 1"));
    assert!(compiled.source.contains("noctalia.log(\"scanned\")"));
}

#[test]
fn rejects_a_void_helper_in_value_position() {
    let handler = r#"
fn helper() {}
fn widget(_ctx: &Context) -> Result<Vec<Op>> {
    let value = helper();
    let _ = value;
    Ok(vec![])
}
"#;
    let error =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect_err("void helper");
    assert!(
        error.message.contains("returns no value"),
        "{}",
        error.message
    );
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
fn compiles_closure_argument_shapes() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let wild = ctx.config_str("message").map(|_| 1);
    let block = ctx.config_str("message").map(|value| { let size = value.len(); size });
    let empty = ctx.config_str("message").map(|_| {});
    let fold = ctx.config_str("message").map_or_else(|| { let fallback = 1; fallback }, |value| { value.len() });
    Ok(vec![Op::log(format!("{}{}{}{}", wild.unwrap_or_default(), block.unwrap_or_default(), empty.unwrap_or_default(), fold))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("if __t1 ~= nil then"));
    assert!(compiled.source.contains("nil"));
}

#[test]
fn compiles_remaining_collection_and_value_methods() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let escaped = "[]-abc".trim_matches("[]-");
    let mut table = std::collections::HashMap::new();
    table.insert("a", 1);
    table.insert(0, 2);
    table.remove("a");
    table.clear();
    let object = json!({ "a": 1 }).as_object().is_some();
    let text = json!("x").as_str().is_some();
    let number = json!(1).as_i64().is_some();
    let boolean = json!(true).as_bool().is_some();
    let array = json!([1]).as_array().is_some();
    let null = json!(null).is_null();
    let millis = ctx.now_ms.duration_since(0).as_millis();
    let secs = ctx.now_ms.as_secs();
    let nanos = ctx.now_ms.as_nanos();
    let parts = "a,b".split(',');
    let first = parts.next().unwrap_or_default();
    let bytes = json!([1]).as_array().unwrap().to_vec();
    Ok(vec![Op::log(format!("{}{}{}{}{}{}{}{}{}{}{}{}{}", escaped, object, text, number, boolean, array, null, millis, secs, nanos, first, bytes.len(), ctx.event))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("string.gsub("));
    assert!(compiled.source.contains("table.clear("));
    assert!(compiled.source.contains("table[0] = 2"));
    assert!(compiled.source.contains("table[\"a\"] = nil"));
    assert!(
        compiled
            .source
            .contains("math.floor(noctalia.nowMs() / 1000)")
    );
    assert!(compiled.source.contains("noctalia.nowMs() * 1000000"));
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
fn scans_loop_and_let_bindings_in_helpers() {
    let handler = r#"
fn helper(ctx: &Context, ops: &mut Vec<Op>) {
    let mut total = 0;
    loop {
        total += 1;
        break;
    }
    if let Some(value) = ctx.config_str("message") {
        total += value.len() as i64;
    }
    total = total + 1;
    ops.push(Op::log(format!("{}", total)));
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    helper(ctx, &mut ops);
    Ok(ops)
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("while true do"));
    assert!(compiled.source.contains("break"));
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
fn compiles_trp_with_the_count_argument() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let localized = ctx.trp("files", 3, &[("name", json!("x"))]);
    let empty = ctx.trp("files", 1, &[]);
    Ok(vec![Op::log(format!("{}{}", localized, empty))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled
            .source
            .contains("noctalia.trp(\"files\", 3, { name = \"x\" })"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("noctalia.trp(\"files\", 1, {"),
        "{}",
        compiled.source
    );

    let short = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let localized = ctx.trp("files", &[]);
    Ok(vec![Op::log(localized)])
}
"#;
    let error =
        compile(&source(short), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(
        error.message.contains("missing argument 2"),
        "{}",
        error.message
    );
    assert!(error.line > 0 && error.column > 0);
}

#[test]
fn compiles_env_var_as_an_optional_string() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let home = std::env::var("HOME").unwrap_or_default();
    let present = std::env::var("MISSING").is_some();
    let fallback = std::env::var("MISSING").unwrap_or("fallback".to_owned());
    Ok(vec![Op::log(format!("{}{}{}", home, present, fallback))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled
            .source
            .contains("(noctalia.getenv(\"HOME\") or \"\")"),
        "{}",
        compiled.source
    );
    assert!(
        compiled
            .source
            .contains("(noctalia.getenv(\"MISSING\") ~= nil)"),
        "{}",
        compiled.source
    );
    assert!(
        compiled
            .source
            .contains("(noctalia.getenv(\"MISSING\") or \"fallback\")"),
        "{}",
        compiled.source
    );
}

#[test]
fn joins_lists_and_bails_on_other_receivers() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let parts = vec!["a".to_owned(), "b".to_owned()];
    let joined = parts.join(",");
    let path = "/tmp".join("file");
    Ok(vec![Op::log(format!("{}{}", joined, path))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("table.concat(parts, \",\")"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("string.sub(__t"),
        "{}",
        compiled.source
    );

    let map = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut table = std::collections::HashMap::new();
    let bad = table.join(",");
    Ok(vec![Op::log(bad)])
}
"#;
    let compiled =
        compile(&source(map), &options("handler", EntryKind::Service))
            .expect("compiles with a runtime join");
    assert!(
        compiled.source.contains("join(table, \",\")"),
        "{}",
        compiled.source
    );
    assert!(
        compiled
            .source
            .contains("local function join(value, separator)")
    );
}

#[test]
fn removes_list_items_and_map_entries() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut values = vec![1, 2, 3];
    let removed = values.remove(0);
    let mut counts = std::collections::HashMap::new();
    counts.insert("a", 1);
    let taken = counts.remove("a");
    Ok(vec![Op::log(format!("{}{}{}", removed, taken.is_some(), values.len()))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("table.remove(values, (0) + 1)"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("counts[\"a\"] = nil"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("(taken ~= nil)"),
        "{}",
        compiled.source
    );

    let set = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut names = std::collections::BTreeSet::new();
    names.remove("a");
    Ok(Vec::new())
}
"#;
    let compiled =
        compile(&source(set), &options("handler", EntryKind::Service))
            .expect("compiles as a map removal");
    assert!(compiled.source.contains("names[\"a\"] = nil"));
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
fn skips_nil_filter_map_results() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let kept = [1, 2, 3].into_iter().filter_map(|value| if value > 1 { Some(value) } else { None }).collect::<Vec<i64>>();
    Ok(vec![Op::log(format!("{}", kept.len()))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("local __t6 = (__t5)"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("if __t6 ~= nil then"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("__t1[#__t1 + 1] = __t6"),
        "{}",
        compiled.source
    );
}

#[test]
fn splits_whitespace_runs_like_rust() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let words = "a\tb".split_whitespace().collect::<Vec<&str>>();
    Ok(vec![Op::log(format!("{}", words.len()))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled
            .source
            .contains("for __word in string.gmatch(\"a\\tb\", \"%S+\")"),
        "{}",
        compiled.source
    );
    assert!(!compiled.source.contains("~= \"\" then"));
}

#[test]
fn collects_only_the_items_left_by_split_next() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut parts = "a.b.c".split('.');
    let first = parts.next().unwrap_or_default();
    let rest = parts.collect::<Vec<&str>>();
    Ok(vec![Op::log(format!("{}{}", first, rest.len()))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("parts.__next = parts.__next + 1"),
        "{}",
        compiled.source
    );
    assert!(
        compiled
            .source
            .contains("for __t5 = __t4.__next or 1, #__t4 do"),
        "{}",
        compiled.source
    );
}

#[test]
fn strips_carriage_returns_and_trailing_newlines() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let lines = "a\r\nb\n".lines().collect::<Vec<&str>>();
    Ok(vec![Op::log(format!("{}", lines.len()))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled
            .source
            .contains("string.gsub(\"a\\r\\nb\\n\", \"\\r\\n\", \"\\n\")"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("table.remove(__lines)"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("string.split(__text, \"\\n\")"),
        "{}",
        compiled.source
    );
}

#[test]
fn loops_over_maps_with_pairs() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut counts = std::collections::HashMap::new();
    counts.insert("a", 1);
    let mut total = 0;
    for (name, value) in counts {
        total += value;
    }
    Ok(vec![Op::log(format!("{}{}", total, name))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled
            .source
            .contains("for name, value in pairs(__t1) do"),
        "{}",
        compiled.source
    );

    let values = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut counts = std::collections::HashMap::new();
    let mut total = 0;
    for value in counts.values() {
        total += value;
    }
    Ok(vec![Op::log(format!("{}", total))])
}
"#;
    let compiled =
        compile(&source(values), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("for _, value in pairs(__t1) do"),
        "{}",
        compiled.source
    );

    let enumerated = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut counts = std::collections::HashMap::new();
    for (index, value) in counts.iter().enumerate() {
        let _ = index;
        let _ = value;
    }
    Ok(Vec::new())
}
"#;
    let error =
        compile(&source(enumerated), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(
        error.message.contains("needs a list, not a map"),
        "{}",
        error.message
    );
    assert!(error.line > 0 && error.column > 0);
}

#[test]
fn compiles_host_callback_ops_with_stable_names() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    if ctx.is_load_event() {
        ops.push(Op::run_stream(json!(["tail", "-f"]), "stream_name"));
        ops.push(Op::run(json!(["echo"]), Some("done_name")));
        ops.push(Op::run(json!(["true"]), None));
        ops.push(Op::http(json!({ "url": "x" }), Some("http_name")));
        ops.push(Op::state_watch("key", "watch_name"));
    }
    if ctx.event == "host_callback" && ctx.payload_str("id").as_deref() == Some("stream_name") {
        let line = ctx.payload_arg(0).and_then(Value::as_str).unwrap_or_default();
        ops.push(Op::log(line.to_owned()));
    }
    Ok(ops)
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let source = &compiled.source;
    assert!(
        source.contains(
            "noctalia.runStream({ \"tail\", \"-f\" }, { __host_callback = \"stream_name\" })"
        ),
        "{source}"
    );
    assert!(
        source.contains("noctalia.runAsync({ \"echo\" }, { __host_callback = \"done_name\" })"),
        "{source}"
    );
    assert!(
        source.contains("noctalia.runAsync({ \"true\" })"),
        "{source}"
    );
    assert!(
        source.contains("noctalia.http({ url = \"x\" }, { __host_callback = \"http_name\" })"),
        "{source}"
    );
    assert!(
        source.contains(
            "state.watch(\"key\", { __host_callback = \"watch_name\" })"
        ),
        "{source}"
    );
    assert!(
        source.contains("function onHostCallback(payload) body_"),
        "{source}"
    );
}

#[test]
fn compiles_ui_call_markers() {
    let handler = r#"
fn widget(_ctx: &Context) -> Result<Vec<Op>> {
    let props = json!({ "onClick": ui::call("onRefresh"), "onRightClick": ui::callback("row") });
    Ok(vec![Op::BarRender { tree: ui::button(props) }])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(
        compiled.source.contains("{ __call = \"onRefresh\" }"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("{ __host_callback = \"row\" }"),
        "{}",
        compiled.source
    );
}

#[test]
fn host_start_methods_compile_only_when_their_result_is_dropped() {
    let dropped = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    ctx.run_async_start(json!(["true"]), Some("cb"), Some(250.0));
    ctx.run_stream_start(json!(["tail"]), "stream");
    ctx.http_start(json!({}), "req");
    Ok(Vec::new())
}
"#;
    let compiled =
        compile(&source(dropped), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains(
            "noctalia.runAsync({ \"true\" }, { __host_callback = \"cb\" }, 250.0)"
        ),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains(
            "noctalia.runStream({ \"tail\" }, { __host_callback = \"stream\" })"
        ),
        "{}",
        compiled.source
    );
    assert!(
        compiled
            .source
            .contains("noctalia.http({  }, { __host_callback = \"req\" })"),
        "{}",
        compiled.source
    );

    let used = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if !ctx.http_start(json!({}), "req") {
        return Ok(Vec::new());
    }
    Ok(Vec::new())
}
"#;
    let error = compile(&source(used), &options("handler", EntryKind::Service))
        .expect_err("bails");
    assert!(error.message.contains("must be discarded"));
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
fn compiles_function_references_as_callbacks() {
    let handler = r#"
fn text(value: Value) -> String {
    value.as_str().unwrap_or_default().to_owned()
}
fn snapshot() -> Value {
    json!({})
}
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let named = ctx.config.get("name").map(text).unwrap_or_default();
    let fallback = ctx.config.get("grid").cloned().unwrap_or_else(snapshot);
    let empty = ctx.config.get("list").and_then(Value::as_array).is_some_and(Vec::is_empty);
    let owned = ctx.config.get("list").and_then(Value::as_array).map_or(Vec::new(), |list| list.clone());
    Ok(vec![Op::log(format!("{named} {empty} {fallback} {owned}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let generated = &compiled.source;
    assert!(generated.contains("local value = __t"), "{generated}");
    assert!(generated.contains("#__value == 0"), "{generated}");
    assert!(generated.contains("local list = __t"), "{generated}");
}

#[test]
fn unit_helpers_compile_inside_closures() {
    let handler = r#"
fn refresh(_ctx: &Context, ops: &mut Vec<Op>) {
    ops.push(Op::log("tick"));
}
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    static INIT: Once = Once::new();
    INIT.call_once(|| refresh(ctx, &mut ops));
    Ok(ops)
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("noctalia.log(\"tick\")"),
        "{}",
        compiled.source
    );
}

#[test]
fn compiles_richer_constants_and_defaults() {
    let handler = r#"
const LIMIT: f64 = 380.0;
const FLOOR: f64 = -1.0;
const TIERS: [(&str, i64); 2] = [("a", 1), ("b", 2)];
struct State { items: Vec<String> }
impl State {
    const EMPTY: State = State { items: Vec::new() };
}
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut total = LIMIT + FLOOR;
    for (name, weight) in TIERS {
        total += weight as f64;
        let _ = name;
    }
    let state = State::EMPTY;
    let capacity = Vec::with_capacity(4);
    let biggest = std::cmp::max(1, 2);
    let ceiling = i64::MAX;
    let smallest = f64::MIN;
    let empty: Map<String, Value> = Map::default();
    Ok(vec![Op::log(format!("{total} {} {} {} {} {}", state.items, capacity.len(), biggest, ceiling, smallest, empty))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let source = &compiled.source;
    assert!(source.contains("380.0"), "{source}");
    assert!(source.contains("(-1.0)"), "{source}");
    assert!(source.contains("9223372036854775807"), "{source}");
    assert!(source.contains("math.max(1, 2)"), "{source}");
    assert!(source.contains("{}"), "{source}");
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
fn compiles_character_sets_and_slices() {
    let handler = r#"
fn is_space(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\n')
}
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let text = ctx.payload_str("text").unwrap_or_default();
    let trimmed = text.trim_matches(is_space);
    let closed = text.trim_matches(|c: char| c.is_ascii_whitespace());
    let listed = text.trim_matches(&[' ', '\t'][..]);
    let head = &text[..2];
    let tail = &text[1..];
    let middle = &text[1..3];
    Ok(vec![Op::log(format!("{trimmed} {closed} {listed} {head} {tail} {middle}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let source = &compiled.source;
    assert!(source.contains("string.sub(text, 1, 2)"), "{source}");
    assert!(
        source.contains("string.sub(text, (1) + 1, #text)"),
        "{source}"
    );
    assert!(source.contains("string.sub(text, (1) + 1, 3)"), "{source}");
}

#[test]
fn compiles_host_queries_and_join() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let dark = ctx.query_bool("noctalia", &["isDarkMode"], Vec::new());
    let value = ctx.query_value("noctalia", &["getSetting"], vec![json!("shell.lang")]);
    let score = ctx.fuzzy_score("abc", "abcdef");
    let dir = ctx.plugin_dir();
    let path = dir.join("icons");
    let parts = ctx.list_dir(&path);
    let joined = parts.join("/");
    Ok(vec![Op::log(format!("{dark} {value} {score} {path} {joined}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let source = &compiled.source;
    assert!(source.contains("local function host_query("), "{source}");
    assert!(
        source.contains("noctalia.fuzzyScore(\"abc\", \"abcdef\")"),
        "{source}"
    );
    assert!(source.contains("table.concat("), "{source}");
}

#[test]
fn read_file_reports_errors_like_a_result() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if let Ok(raw) = ctx.read_file("/tmp/x") {
        return Ok(vec![Op::log(raw)]);
    }
    let Ok(exact) = ctx.read_file("/tmp/y") else {
        return Ok(Vec::new());
    };
    Ok(vec![Op::log(exact)])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let source = &compiled.source;
    assert!(
        source.contains("local __t1, __t2 = noctalia.readFile(\"/tmp/x\")"),
        "{source}"
    );
    assert!(source.contains("local exact = __t"), "{source}");
}

#[test]
fn compiles_map_removals_and_unit_defaults() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut queries: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
    queries.insert("a".to_owned(), json!(1));
    let removed = queries.remove("a");
    let empty: std::collections::HashMap<String, Value> = Default::default();
    Ok(vec![Op::log(format!("{removed} {empty}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("= nil"), "{}", compiled.source);
}

#[test]
fn compiles_iterator_aggregates() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let items = ctx.config.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
    let any = items.iter().any(|item| item.is_string());
    let all = items.iter().all(|item| item.is_string());
    let sum: f64 = items.len() as f64;
    let count = items.iter().count();
    let mut seen = 0;
    items.iter().for_each(|_| { seen += 1 });
    Ok(vec![Op::log(format!("{any} {all} {sum} {count} {seen}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let source = &compiled.source;
    assert!(source.contains("= true"), "{source}");
    assert!(source.contains("= false"), "{source}");
    assert!(source.contains("break"), "{source}");
}

#[test]
fn compiles_the_remaining_string_and_number_methods() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let text = ctx.payload_str("text").unwrap_or_default();
    let mut list = vec![text.clone()];
    let first = list.first().cloned().unwrap_or_default();
    list.push_back("tail".to_owned());
    let popped = list.pop().unwrap_or_default();
    let found = text.find("|").unwrap_or(0);
    let nan = (0.0f64 / 0.0).is_nan();
    let quotient = 7.0.div_euclid(2.0);
    let rounded = 1.5f64.round();
    let optional = ctx.config.get("name").cloned();
    let chosen = optional.clone().or(Some(json!("x")));
    let flag = optional.clone().is_none_or(|value| value.is_string());
    let upper = text.is_ascii_uppercase();
    let alnum = text.is_ascii_alphanumeric();
    let digit = text.is_ascii_digit();
    let space = text.is_whitespace();
    let started = text.trim_start();
    let ended = text.trim_end();
    let suffix = text.strip_suffix(".txt").unwrap_or_default();
    let bytes = text.as_bytes();
    let owned: String = text.clone().into();
    let sized = bytes.len();
    let pushed = pushed_value(text);
    Ok(vec![Op::log(format!("{first}{popped}{found}{nan}{quotient}{rounded}{chosen}{flag}{upper}{alnum}{digit}{space}{started}{ended}{suffix}{sized}{pushed}"))])
}
fn pushed_value(text: String) -> String {
    let mut buffer = String::new();
    buffer.push_str(&text);
    buffer
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let source = &compiled.source;
    assert!(source.contains("table.remove(list)"), "{source}");
    assert!(
        source.contains("string.find(text, \"|\", 1, true)"),
        "{source}"
    );
    assert!(source.contains("math.floor(7.0 / 2.0)"), "{source}");
    assert!(
        source.contains("~= nil then return")
            && source.contains("tostring(chosen)"),
        "{source}"
    );
}

#[test]
fn compiles_character_set_variants() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let text = ctx.payload_str("text").unwrap_or_default();
    let bytes = text.trim_matches(b' ');
    let packed = text.trim_matches(b" \t");
    let digits = text.trim_matches(|c: char| c.is_ascii_digit());
    let matched = text.trim_matches(|c: char| matches!(c, '-' | '+'));
    let wrapped = text.trim_matches(&['^', ']', '%', '-'][..]);
    Ok(vec![Op::log(format!("{bytes}{packed}{digits}{matched}{wrapped}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let source = &compiled.source;
    assert!(source.contains("[%^%]%%%-]"), "{source}");
    assert!(source.contains("0123456789"), "{source}");
    assert!(source.contains("[%-+]"), "{source}");
}

#[test]
fn bails_on_character_sets_it_cannot_read() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let text = ctx.payload_str("text").unwrap_or_default();
    let unknown = text.trim_matches(|c: char| c == 'a' && c != 'b');
    Ok(vec![Op::log(unknown)])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(
        error.message.contains("string or char literal"),
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
fn compiles_container_callbacks_and_bounds() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let list = ctx.config.get("list").and_then(Value::as_array).cloned();
    let owned = list.clone().map_or(Vec::new(), Vec::as_slice);
    let fresh = list.clone().unwrap_or_else(Map::new);
    let default = list.clone().unwrap_or_else(Vec::new);
    let empty = list.clone().is_some_and(Vec::is_empty);
    let bounded = ctx.config.get("n").and_then(Value::as_f64).unwrap_or(i64::MAX as f64);
    let endless = f64::MAX;
    let lowest = i64::MIN;
    let minimal = f64::MIN;
    Ok(vec![Op::log(format!("{owned}{fresh}{default}{empty}{bounded}{endless}{lowest}{minimal}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let source = &compiled.source;
    assert!(!source.contains("math.huge"), "{source}");
    assert!(source.contains("1.7976931348623157e308"), "{source}");
    assert!(source.contains("9223372036854775807"), "{source}");
}

#[test]
fn bails_on_open_ended_list_slices() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let list = ctx.config.get("list").and_then(Value::as_array).cloned().unwrap_or_default();
    let tail = &list[1..];
    Ok(vec![Op::log(format!("{tail:?}"))])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(
        error.message.contains("string receiver"),
        "{}",
        error.message
    );
}

#[test]
fn compiles_glob_imported_constants_and_associated_values() {
    let main = r#"
mod state;
use state::*;
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = STATE_KEY;
    let preset = Preset::EMPTY;
    Ok(vec![Op::log(format!("{value} {preset}"))])
}
"#;
    let state = r#"
pub const STATE_KEY: &str = "me.state";
pub struct Preset { pub name: String }
impl Preset {
    pub const EMPTY: Preset = Preset { name: String::new() };
}
"#;
    let modules = [Module {
        path: "state",
        source: state,
    }];
    let compiled = compile(
        &source(main),
        &module_options("handler", EntryKind::Service, &modules),
    )
    .expect("compiles");
    assert!(compiled.source.contains("me.state"), "{}", compiled.source);
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
fn compiles_exact_integer_and_float_bounds() {
    let bounds = [
        ("i8::MAX", "127"),
        ("i8::MIN", "(-128)"),
        ("i16::MAX", "32767"),
        ("i16::MIN", "(-32768)"),
        ("i32::MAX", "2147483647"),
        ("i32::MIN", "(-2147483648)"),
        ("i64::MAX", "9223372036854775807"),
        ("i64::MIN", "(-9223372036854775808)"),
        ("i128::MAX", "170141183460469231731687303715884105727"),
        ("u8::MAX", "255"),
        ("u16::MAX", "65535"),
        ("u32::MAX", "4294967295"),
        ("u64::MAX", "18446744073709551615"),
        ("u128::MAX", "340282366920938463463374607431768211455"),
        ("usize::MAX", "18446744073709551615"),
        ("isize::MAX", "9223372036854775807"),
        ("u8::MIN", "0"),
        ("usize::MIN", "0"),
        ("f64::MAX", "1.7976931348623157e308"),
        ("f64::MIN", "(-1.7976931348623157e308)"),
        ("f32::MAX", "3.4028234663852886e38"),
        ("f32::MIN", "(-3.4028234663852886e38)"),
    ];
    for (path, expected) in bounds {
        let handler = format!(
            r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {{
    Ok(vec![Op::log(format!("{{}}", {path}))])
}}
"#
        );
        let compiled =
            compile(&source(&handler), &options("handler", EntryKind::Service))
                .expect("compiles");
        assert!(
            compiled.source.contains(expected),
            "{path} should emit {expected}: {}",
            compiled.source
        );
        assert!(
            !compiled.source.contains("math.huge"),
            "{path}: {}",
            compiled.source
        );
    }
}

#[test]
fn compiles_typed_defaults_from_the_type_hint() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let name: String = Default::default();
    let count: u32 = Default::default();
    let flag: bool = Default::default();
    let items: Vec<Value> = Default::default();
    Ok(vec![Op::log(format!("{name}{count}{flag}{items}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let emitted = &compiled.source;
    assert!(emitted.contains("local name = \"\""), "{emitted}");
    assert!(emitted.contains("local count = 0"), "{emitted}");
    assert!(emitted.contains("local flag = false"), "{emitted}");
    assert!(emitted.contains("local items = {}"), "{emitted}");

    let untyped = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let unknown = Default::default();
    Ok(vec![Op::log(format!("{unknown}"))])
}
"#;
    let error =
        compile(&source(untyped), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(
        error.message.contains("needs a known type annotation"),
        "{}",
        error.message
    );
    assert!(error.line > 0 && error.column > 0, "{error:?}");
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
fn compiles_option_or_with_an_explicit_nil_check() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let flag = ctx.config.get("flag").and_then(Value::as_bool);
    let chosen = flag.or(Some(false));
    Ok(vec![Op::log(format!("{chosen}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let source = &compiled.source;
    assert!(!source.contains("(flag or "), "{source}");
    assert!(source.contains("~= nil then return"), "{source}");
    assert!(source.contains("return false end)()"), "{source}");
}

#[test]
fn bails_on_char_boundary_and_char_indices() {
    let boundary = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let text = ctx.payload_str("text").unwrap_or_default();
    let inside = text.is_char_boundary(1);
    Ok(vec![Op::log(format!("{inside}"))])
}
"#;
    let error =
        compile(&source(boundary), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(
        error.message.contains("is_char_boundary"),
        "{}",
        error.message
    );

    let indices = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let text = ctx.payload_str("text").unwrap_or_default();
    for (at, ch) in text.char_indices() {
        let _ = (at, ch);
    }
    Ok(vec![Op::log(text)])
}
"#;
    let error =
        compile(&source(indices), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(error.message.contains("char_indices"), "{}", error.message);
}

#[test]
fn bails_on_list_range_indexes() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let list = ctx.config.get("list").and_then(Value::as_array).cloned().unwrap_or_default();
    let part = &list[1..3];
    Ok(vec![Op::log(format!("{part}"))])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(
        error.message.contains("string receiver"),
        "{}",
        error.message
    );
    assert!(error.line > 0 && error.column > 0, "{error:?}");
}

#[test]
fn compiles_then_some_with_an_eager_value() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let ready = true;
    let picked = ready.then_some("yes".to_owned());
    Ok(vec![Op::log(format!("{picked}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let source = &compiled.source;
    let value = source.find("local __t1 = \"yes\"").expect("value temp");
    let condition = source.find("if ready then return __t1").expect("guard");
    assert!(value < condition, "{source}");
}

#[test]
fn compiles_exact_iterator_and_string_methods() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let values = vec![1, 2, 3];
    let found = values.iter().find(|value| **value == 2).is_some();
    let position = values.iter().position(|value| **value == 3).unwrap_or(0);
    let tail: Vec<i64> = values.iter().skip(1).take(1).copied().collect();
    let parts: Vec<&str> = "a/b/c".rsplit('/').collect();
    let pair = "name=value".split_once('=').is_some();
    let last = "left:right".rfind(':').unwrap_or(0);
    let mut header = vec!["a".to_owned()];
    header.extend_from_slice(&["b".to_owned()]);
    let tail_len = tail.len();
    let parts_len = parts.len();
    let header_len = header.len();
    Ok(vec![Op::log(format!("{found} {position} {tail_len} {parts_len} {pair} {last} {header_len}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("break"), "{}", compiled.source);
    assert!(
        compiled.source.contains("string.split"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("table.insert"),
        "{}",
        compiled.source
    );
}

#[test]
fn compiles_flatten_math_and_character_methods() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let nested: Option<Vec<i64>> = Some(vec![1, 2]);
    let mut total = 0;
    for value in nested.into_iter().flatten() {
        total += value;
    }
    let text = "abc";
    let hex = text.bytes().all(|byte| byte.is_ascii_hexdigit());
    let wave = (1.0_f64).sin() + (1.0_f64).cos();
    let infinite = wave.is_infinite();
    let whole = wave.trunc();
    let mut rows = vec![1, 2, 3];
    rows.retain(|value| *value > 1);
    rows.truncate(1);
    let rows_len = rows.len();
    Ok(vec![Op::log(format!("{total} {hex} {infinite} {whole} {rows_len}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("math.sin"), "{}", compiled.source);
    assert!(compiled.source.contains("math.cos"), "{}", compiled.source);
    assert!(
        compiled.source.contains("table.remove"),
        "{}",
        compiled.source
    );
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
fn bails_on_comparators_and_type_erased_maps() {
    for handler in [
        "fn handler(_ctx: &Context) -> Result<Vec<Op>> { let mut values = vec![2, 1]; values.sort_by(|a, b| a.cmp(b)); Ok(vec![]) }",
        "fn handler(_ctx: &Context) -> Result<Vec<Op>> { let cell = once_cell(); cell.get_or_init(|| 1); Ok(vec![]) }",
    ] {
        let error =
            compile(&source(handler), &options("handler", EntryKind::Service))
                .expect_err("bail");
        assert!(error.line > 0 && error.column > 0, "{error:?}");
    }
}

#[test]
fn compiles_tuple_loops_closure_patterns_and_dynamic_translation_keys() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let _ = ctx;
    let rows = ctx.state.get("rows").unwrap_or(&serde_json::Value::Null);
    let mut total = 0;
    for (key, value) in rows {
        total += value.as_i64().unwrap_or(0);
    }
    let pairs = [("a", 1), ("b", 2)];
    let found = pairs.iter().find(|(key, _)| *key == "a").is_some();
    let label = ctx.tr(ctx.config_str("message").unwrap_or_default(), &[]);
    Ok(vec![Op::log(format!("{total} {found} {label}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("type(") && compiled.source.contains("pairs("),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("noctalia.tr("),
        "{}",
        compiled.source
    );
    assert!(compiled.source.contains("[1]"), "{}", compiled.source);
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
fn compiles_transparent_tuple_structs() {
    let handler = r#"
struct Score(pub i64);
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let score = Score(3);
    let value = score.0;
    Ok(vec![Op::log(format!("{value}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let generated = &compiled.source;
    assert!(generated.contains("local score = 3"), "{generated}");
    assert!(generated.contains("local value = score"), "{generated}");
    assert!(!generated.contains("score[1]"), "{generated}");
}

#[test]
fn compiles_empty_vector_constructors() {
    let handler = r#"
use std::collections::VecDeque;
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let list = Vec::<i64>::new();
    let queue = VecDeque::<i64>::new();
    Ok(vec![Op::log(format!("{}{}", list.len(), queue.len()))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let generated = &compiled.source;
    assert!(generated.contains("local list = {}"), "{generated}");
    assert!(generated.contains("local queue = {}"), "{generated}");
}

#[test]
fn bails_on_array_initializers_and_comparators_with_positions() {
    for (handler, name) in [
        (
            "fn handler(_ctx: &Context) -> Result<Vec<Op>> { let values = std::array::from_fn(|_| 1); Ok(vec![]) }",
            "from_fn",
        ),
        (
            "fn handler(_ctx: &Context) -> Result<Vec<Op>> { let mut values = vec![2, 1]; values.sort_by(|a, b| a.cmp(b)); Ok(vec![]) }",
            "sort_by",
        ),
    ] {
        let error =
            compile(&source(handler), &options("handler", EntryKind::Service))
                .expect_err("bail");
        assert!(error.line > 0 && error.column > 0, "{error:?}");
        assert!(
            error.render("main.rs").starts_with(&format!(
                "main.rs:{}:{}:",
                error.line, error.column
            )),
            "{error:?}"
        );
        assert!(error.message.contains(name), "{}", error.message);
    }
}

#[test]
fn bails_on_luau_keyword_identifiers() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let end = 1;
    Ok(vec![Op::log(format!("{end}"))])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("bail");
    assert!(error.message.contains("Luau keyword"), "{}", error.message);
    assert!(error.line > 0 && error.column > 0, "{error:?}");
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
fn flattens_a_list_of_lists_into_one_list() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let nested: Vec<Vec<i64>> = vec![vec![1, 2], vec![3]];
    let mut total = 0;
    for value in nested.into_iter().flatten() {
        total += value;
    }
    Ok(vec![Op::log(format!("{total}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let generated = &compiled.source;
    assert!(
        generated.contains("for __j = 1, #__inner do"),
        "{generated}"
    );
    assert!(generated.contains("__out.__next = 1"), "{generated}");
}

#[test]
fn bails_on_find_after_flatten() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let nested: Vec<Vec<i64>> = vec![vec![1, 2], vec![3]];
    let found = nested.iter().flatten().find(|value| **value == 3).is_some();
    Ok(vec![Op::log(format!("{found}"))])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("bail");
    assert!(error.message.contains("flatten"), "{}", error.message);
    assert!(error.line > 0 && error.column > 0, "{error:?}");
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

#[test]
fn unwraps_newtype_field_access_on_a_constructor_call() {
    let handler = r#"
struct Score(pub i64);
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = Score(4).0;
    Ok(vec![Op::log(format!("{value}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let generated = &compiled.source;
    assert!(generated.contains("local value = 4"), "{generated}");
    assert!(!generated.contains("[1]"), "{generated}");
}

#[test]
fn checks_hex_digits_on_byte_receivers() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let byte = 65_u8;
    let hex = byte.is_ascii_hexdigit();
    Ok(vec![Op::log(format!("{hex}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let generated = &compiled.source;
    assert!(generated.contains("local __byte = byte"), "{generated}");
    assert!(generated.contains("__byte >= 48"), "{generated}");
    assert!(!generated.contains("string.find(byte"), "{generated}");
}

#[test]
fn compiles_vector_constructors_with_capacity() {
    let handler = r#"
use std::collections::VecDeque;
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut list = Vec::<i64>::with_capacity(4);
    list.push(1);
    let queue = VecDeque::<i64>::with_capacity(2);
    let sizes = (list.len(), queue.len());
    Ok(vec![Op::log(format!("{}{}", sizes.0, sizes.1))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let generated = &compiled.source;
    assert!(generated.contains("local list = {}"), "{generated}");
    assert!(generated.contains("local queue = {}"), "{generated}");
}

fn native_options<'a>(handler: &'a str, kind: EntryKind) -> Options<'a> {
    Options {
        handler,
        entry: "widget",
        state_key: "hello.state",
        kind,
        strings: &["message", "name"],
        module: "main",
        modules: &[],
        native: NativeMode::Allow,
    }
}

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

const NATIVE_HELPER: &str = r#"
use noctalia_rs::{Context, Op, Result};

fn load_rows() -> Result<Vec<String>> {
    let text = std::fs::read_to_string("/tmp/rows")?;
    Ok(vec![text])
}

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let rows = load_rows()?;
    Ok(vec![Op::log(rows.join(","))])
}
"#;

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
    assert_eq!(native.params, Vec::<String>::new());
    assert!(native.reason.contains("std::fs"), "{}", native.reason);
    assert!(
        compiled.source.contains(
            "local function __native_fn_main__load__rows()\n  return __native.call(\"main::load_rows\", {}, 0)\nend"
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
    assert_eq!(compiled.native[0].params, vec!["value".to_owned()]);
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
    let text = std::fs::read_to_string("/tmp/rows")?;
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
fn native_signatures_reject_references_and_self() {
    let reference = r#"
fn load_rows(path: &str) -> Result<Vec<String>> {
    let text = std::fs::read_to_string(path)?;
    Ok(vec![text])
}

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let rows = load_rows("/tmp/rows")?;
    Ok(vec![Op::log(rows.join(","))])
}
"#;
    let error = compile(
        &source(reference),
        &native_options("handler", EntryKind::Widget),
    )
    .expect_err("references cannot cross the boundary");
    assert!(error.message.contains("cannot be references"), "{error}");

    let receiver = r#"
struct Store {
    value: i64,
}
impl Store {
    fn load_rows(&self) -> Result<Vec<String>> {
        let text = std::fs::read_to_string("/tmp/rows")?;
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
    let text = std::fs::read_to_string("/tmp/rows")?;
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
    let text = std::fs::read_to_string("/tmp/rows")?;
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
