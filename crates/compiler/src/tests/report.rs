use super::*;

#[test]
fn report_counts_compiled_bodies_and_generated_bytes() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::log(ctx.config_str("message").unwrap_or_default().into_owned())])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");

    assert_eq!(compiled.report.generated_bytes, compiled.source.len());
    assert_eq!(compiled.report.native_functions, 0);
    assert_eq!(
        compiled.report.compiled_functions,
        compiled.source.matches("local function body_").count()
    );
    assert!(
        compiled
            .report
            .helper_calls_per_event
            .contains_key("update")
    );
    assert!(compiled.report.helper_calls_per_event.contains_key("load"));
    assert!(
        compiled
            .report
            .helper_calls_per_event
            .values()
            .all(|calls| *calls == 0)
    );
}

#[test]
fn report_counts_native_calls_in_each_event_body() {
    let handler = r#"
fn load_rows() -> Result<Vec<String>> {
    let text = std::fs::read_link("/tmp/rows")?.to_string_lossy().into_owned();
    Ok(vec![text])
}

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    match ctx.event.as_str() {
        "click" => {
            let first = load_rows()?;
            let second = load_rows()?;
            Ok(vec![Op::log(first.join(",")), Op::log(second.join(","))])
        }
        _ => {
            let rows = load_rows()?;
            Ok(vec![Op::log(rows.join(","))])
        }
    }
}
"#;
    let compiled = compile(
        &source(handler),
        &native_options("handler", EntryKind::Widget),
    )
    .expect("auto compiles");

    assert_eq!(compiled.report.native_functions, 1);
    assert_eq!(
        compiled.report.compiled_functions,
        compiled.source.matches("local function body_").count()
    );
    assert_eq!(
        compiled.report.helper_calls_per_event.get("render"),
        Some(&1),
        "{:?}",
        compiled.report.helper_calls_per_event
    );
    assert_eq!(
        compiled.report.helper_calls_per_event.get("click"),
        Some(&2),
        "{:?}",
        compiled.report.helper_calls_per_event
    );
}

#[test]
fn report_tracks_the_final_script_after_native_discovery() {
    let compiled = compile(
        &source(NATIVE_HELPER),
        &native_options("handler", EntryKind::Widget),
    )
    .expect("auto compiles");

    assert_eq!(compiled.report.native_functions, compiled.native.len());
    assert_eq!(compiled.report.generated_bytes, compiled.source.len());
    assert_eq!(
        compiled.report.helper_calls_per_event.get("render"),
        Some(&1),
        "{:?}",
        compiled.report.helper_calls_per_event
    );
}
