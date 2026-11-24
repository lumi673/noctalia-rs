use crate::{EntryKind, Options, compile};

fn options<'a>(handler: &'a str, kind: EntryKind) -> Options<'a> {
    Options {
        handler,
        entry: "widget",
        state_key: "hello.state",
        kind,
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

    assert!(
        compiled.source.contains(
            "if ((event == \"render\") or (event == \"click\")) then"
        )
    );
    assert!(
        compiled.source.contains(
            "local message = (noctalia.getConfig(\"message\") or \"\")"
        )
    );
    assert!(compiled.source.contains("barWidget.setText(message)"));
    assert!(compiled.source.contains("function onClick()"));
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
    assert!(compiled.source.contains("body(\"load\""));
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
            .contains("local state = noctalia.state.get(STATE_KEY) or {}")
    );
    assert!(
        compiled
            .source
            .contains("local label = (state[\"label\"] or \"\")")
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

    assert!(
        compiled
            .source
            .contains("if event == \"query\" or event == \"search\" then")
    );
    assert!(
        compiled
            .source
            .contains("elseif event == \"activate\" then")
    );
    assert!(compiled.source.contains("function onQuery(text)"));
    assert!(compiled.source.contains("function onSearch(text)"));
    assert!(compiled.source.contains("function onActivate(id)"));
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
