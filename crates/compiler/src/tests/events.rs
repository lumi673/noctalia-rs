use crate::{EntryKind, compile};

use super::{options, source};

#[test]
fn typed_match_dispatches_every_variant_and_decodes_payload() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    match ctx.widget_event() {
        WidgetEvent::Scroll(scroll) => {
            let _ = scroll;
            Ok(vec![Op::log("scroll")])
        }
        WidgetEvent::Click => Ok(vec![Op::log("click")]),
        _ => Ok(Vec::new()),
    }
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Widget))
            .expect("compiles");
    assert_eq!(compiled.events, vec!["click", "scroll"]);
    for needle in [
        "local function body_scroll(payload)",
        "local scroll = payload",
        "function onScroll(axis, steps, startsGesture)",
        "function onClick() body_click() end",
    ] {
        assert!(
            compiled.source.contains(needle),
            "missing {needle}\n{}",
            compiled.source
        );
    }
}

#[test]
fn typed_payload_fields_support_option_methods() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    match ctx.widget_event() {
        WidgetEvent::Scroll(scroll) => {
            let axis = scroll.axis.unwrap_or_default();
            Ok(vec![Op::log(axis)])
        }
        _ => Ok(Vec::new()),
    }
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Widget))
            .expect("compiles");
    assert!(
        compiled.source.contains("local scroll = payload"),
        "{}",
        compiled.source
    );
}

#[test]
fn typed_unknown_arm_binding_receives_the_event_name() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "mystery" {
        return match ctx.widget_event() {
            WidgetEvent::Unknown(name) => Ok(vec![Op::log(name)]),
        };
    }
    Ok(Vec::new())
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Widget))
            .expect("compiles");
    assert!(
        compiled.source.contains("local name = \"mystery\""),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("noctalia.log(name)"),
        "{}",
        compiled.source
    );
}

#[test]
fn matches_macro_accepts_typed_event_variants() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if matches!(ctx.widget_event(), WidgetEvent::Click) {
        Ok(vec![Op::log("click")])
    } else {
        Ok(Vec::new())
    }
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.events.contains(&"click".to_owned()));
    assert!(
        compiled
            .source
            .contains("function onClick() body_click() end"),
        "{}",
        compiled.source
    );
}

#[test]
fn matches_macro_unknown_only_emits_false_for_known_labels() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if matches!(ctx.widget_event(), WidgetEvent::Unknown(_)) {
        Ok(vec![Op::log("unknown")])
    } else {
        Ok(Vec::new())
    }
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Widget))
            .expect("compiles");
    assert!(
        compiled.source.contains("if (false) then"),
        "{}",
        compiled.source
    );
    assert!(
        !compiled.source.contains("if () then"),
        "{}",
        compiled.source
    );
    assert!(
        compiled
            .source
            .contains("function onClick() body_render() end"),
        "{}",
        compiled.source
    );
    assert!(
        !compiled.source.contains("function onClick() body_events"),
        "{}",
        compiled.source
    );
}
#[test]
fn typed_match_folds_variant_comparisons_in_every_mode() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.panel_event() == PanelEvent::Click {
        Ok(vec![Op::log("click")])
    } else {
        Ok(Vec::new())
    }
}
"#;
    for kind in [EntryKind::Panel, EntryKind::Layer] {
        let compiled = compile(&source(handler), &options("handler", kind))
            .expect("compiles");
        assert!(compiled.events.contains(&"click".to_owned()));
        assert!(
            compiled
                .source
                .contains("function onClick() body_click() end"),
            "missing click dispatch\n{}",
            compiled.source
        );
    }
}

#[test]
fn typed_match_fails_closed_when_a_label_has_no_arm() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    match ctx.widget_event() {
        WidgetEvent::Click => Ok(vec![Op::log("click")]),
        WidgetEvent::Unknown(_) => Ok(Vec::new()),
    }
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Widget))
            .expect_err("must fail");
    let rendered = error.render("widget.rs");
    assert!(
        rendered.contains("typed event match has no arm for `render`"),
        "{rendered}"
    );
    assert!(rendered.starts_with("widget.rs:"), "{rendered}");
}

#[test]
fn typed_match_rejects_unknown_variants_with_a_position() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    match ctx.widget_event() {
        WidgetEvent::Nope => Ok(Vec::new()),
        _ => Ok(Vec::new()),
    }
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Widget))
            .expect_err("must fail");
    let rendered = error.render("widget.rs");
    assert!(
        rendered.contains("typed event variant `Nope` is not recognized"),
        "{rendered}"
    );
    assert!(rendered.starts_with("widget.rs:"), "{rendered}");
}

#[test]
fn typed_match_rejects_guards_with_a_position() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    match ctx.service_event() {
        ServiceEvent::Update(update) if update.source.is_some() => Ok(Vec::new()),
        _ => Ok(Vec::new()),
    }
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("must fail");
    let rendered = error.render("service.rs");
    assert!(
        rendered.contains("guards are not supported in typed event matches"),
        "{rendered}"
    );
    assert!(rendered.starts_with("service.rs:"), "{rendered}");
}
