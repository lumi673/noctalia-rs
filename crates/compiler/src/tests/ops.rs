use super::*;

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
            .contains("type(row[\"value\"]) == \"number\"")
    );
    assert!(compiled.source.contains("row[\"value\"] % 1 == 0"));
    assert!(compiled.source.contains("== nil then __t"));
    assert!(compiled.source.contains("total += value"));
    assert!(compiled.source.contains("barWidget.setText(status)"));
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
        "host_call(\"noctalia\", { \"systemStats\" }, setmetatable({}, { __noctalia_array = true }))",
    ] {
        assert!(
            compiled.source.contains(needle),
            "missing {needle}\n{}",
            compiled.source
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
        compiled.source.contains("for __key in pairs(counts) do"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("table.sort(__keys"),
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
