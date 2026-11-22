mod desktop;
mod duration;
mod launcher;
mod layer;
mod panel;
mod service;
mod shortcut;
mod timer;
mod widget;

use std::process::ExitCode;

use noctalia::run as run_cli;
use noctalia::{Context, Entry, EntryKind, MetaBuilder, Op, Plugin, Setting};
use serde_json::json;

use self::timer::{Store, Timer};

pub const PANEL_ID: &str = "iblameyou/example:panel";
pub const STATE_KEY: &str = "example.state";

const COLORS: &[(&str, &str)] = &[
    ("primary", "settings.color.primary"),
    ("secondary", "settings.color.secondary"),
    ("tertiary", "settings.color.tertiary"),
    ("error", "settings.color.error"),
    ("on_surface", "settings.color.on_surface"),
    ("on_surface_variant", "settings.color.on_surface_variant"),
];

const TRANSLATIONS: &[(&str, &str)] = &[
    ("notify.title", "Example"),
    ("notify.body", "{duration} timer finished"),
    ("bar.empty", "No timers"),
    ("bar.timer", "{duration} timer"),
    ("bar.paused", "Paused - {time}"),
    ("panel.title", "Example"),
    ("panel.empty", "No timers running"),
    ("panel.timer", "{duration} timer"),
    ("panel.ends_at", "Ends at {time}"),
    ("panel.paused_left", "Paused"),
    ("panel.pause", "Pause"),
    ("panel.resume", "Resume"),
    ("panel.stop", "Stop"),
    ("panel.stop_all", "Stop all"),
    ("panel.placeholder", "Duration, e.g. 1h5 or 5"),
    ("panel.start", "Start"),
    ("panel.invalid", "Invalid duration"),
    ("launcher.hint", "Try 1h5, 1y, 4s10h3m, or 5 for 5 minutes"),
    ("launcher.start", "Start timer {time}"),
    ("launcher.start_new", "Start a new timer"),
    ("launcher.timer", "{duration} timer"),
    ("launcher.message", "{duration} - {message}"),
    ("launcher.stop", "Stop"),
    ("launcher.pause", "Pause"),
    ("launcher.resume", "Resume"),
    ("launcher.no_matching", "No timers here"),
    ("launcher.ends_at", "Ends at {time} - {duration}"),
    ("launcher.paused_at", "Paused - {duration} left"),
    ("launcher.stop_all", "Stop all timers"),
    ("launcher.active_count", "{count} timers"),
    ("launcher.invalid", "Invalid duration"),
    ("settings.show_text.label", "Show time"),
    (
        "settings.show_text.description",
        "Show the remaining time next to the ring.",
    ),
    ("settings.show_count.label", "Show count"),
    (
        "settings.show_count.description",
        "Show the number of timers inside the ring.",
    ),
    ("settings.show_progress.label", "Show progress ring"),
    (
        "settings.show_progress.description",
        "Draw a progress ring for the nearest timer.",
    ),
    ("settings.use_urgency_colors.label", "Timer colors"),
    (
        "settings.use_urgency_colors.description",
        "Change color as the timer runs out.",
    ),
    ("settings.safe_color.label", "Safe color"),
    (
        "settings.safe_color.description",
        "Ring color while more than half remains.",
    ),
    ("settings.warning_color.label", "Warning color"),
    (
        "settings.warning_color.description",
        "Ring color when half or less remains.",
    ),
    ("settings.urgent_color.label", "Urgent color"),
    (
        "settings.urgent_color.description",
        "Ring color when a fifth or less remains.",
    ),
    ("settings.idle_color.label", "Idle color"),
    (
        "settings.idle_color.description",
        "Ring color when no timers are running.",
    ),
    ("settings.hide_when_idle.label", "Hide when idle"),
    (
        "settings.hide_when_idle.description",
        "Hide the widget when no timers are running.",
    ),
    ("settings.color.primary", "Primary"),
    ("settings.color.secondary", "Secondary"),
    ("settings.color.tertiary", "Tertiary"),
    ("settings.color.error", "Error"),
    ("settings.color.on_surface", "Text"),
    ("settings.color.on_surface_variant", "Muted text"),
];

#[must_use]
pub fn publish(store: &Store) -> Op {
    Op::state(STATE_KEY, json!({ "timers": store.timers }))
}

#[must_use]
pub fn timers_from_state(ctx: &Context) -> Vec<Timer> {
    ctx.state_value("timers").unwrap_or_default()
}

#[must_use]
pub fn add_timer(
    ctx: &Context,
    spec: (&mut Store, u64, Option<String>),
) -> bool {
    let (store, seconds, message) = spec;

    if seconds == 0 {
        return false;
    }

    store.seq = store.seq.saturating_add(1);
    let offset = i64::try_from(seconds)
        .unwrap_or(i64::MAX)
        .saturating_mul(1_000);

    store.timers.push(Timer {
        id: format!("{}-{}", ctx.now_ms, store.seq),
        seconds,
        label: duration::human(seconds),
        message,
        paused: false,
        ends_at: Some(ctx.now_ms.saturating_add(offset)),
        remaining_ms: None,
    });
    timer::sort(store);

    true
}

#[must_use]
pub fn stop_by_id(store: &mut Store, id: &str) -> bool {
    let before = store.timers.len();
    store.timers.retain(|timer| timer.id != id);
    store.timers.len() != before
}

#[must_use]
pub fn pause_by_id(ctx: &Context, store: &mut Store, id: &str) -> bool {
    let Some(timer) = store
        .timers
        .iter_mut()
        .find(|timer| timer.id == id && !timer.paused)
    else {
        return false;
    };

    let remaining = timer
        .ends_at
        .map_or(0, |end| end.saturating_sub(ctx.now_ms));
    if remaining <= 0 {
        return false;
    }

    timer.paused = true;
    timer.remaining_ms = Some(u64::try_from(remaining).unwrap_or(0));
    timer.ends_at = None;
    timer::sort(store);

    true
}

#[must_use]
pub fn resume_by_id(ctx: &Context, store: &mut Store, id: &str) -> bool {
    let Some(timer) = store
        .timers
        .iter_mut()
        .find(|timer| timer.id == id && timer.paused)
    else {
        return false;
    };

    let remaining = timer.remaining_ms.unwrap_or(0);
    timer.paused = false;
    timer.ends_at = Some(
        ctx.now_ms
            .saturating_add(i64::try_from(remaining).unwrap_or(0)),
    );
    timer.remaining_ms = None;
    timer::sort(store);

    true
}

fn settings() -> Vec<Setting> {
    let colors: Vec<(&str, &str)> = COLORS.to_vec();

    vec![
        Setting::bool("show_text", "settings.show_text.label", true)
            .description("settings.show_text.description"),
        Setting::bool("show_count", "settings.show_count.label", true)
            .description("settings.show_count.description"),
        Setting::bool("show_progress", "settings.show_progress.label", true)
            .description("settings.show_progress.description"),
        Setting::bool("hide_when_idle", "settings.hide_when_idle.label", false)
            .description("settings.hide_when_idle.description"),
        Setting::bool(
            "use_urgency_colors",
            "settings.use_urgency_colors.label",
            true,
        )
        .description("settings.use_urgency_colors.description"),
        Setting::select(
            "safe_color",
            "settings.safe_color.label",
            ("tertiary", &colors),
        )
        .description("settings.safe_color.description"),
        Setting::select(
            "warning_color",
            "settings.warning_color.label",
            ("secondary", &colors),
        )
        .description("settings.warning_color.description")
        .visible_when("use_urgency_colors", &["true"]),
        Setting::select(
            "urgent_color",
            "settings.urgent_color.label",
            ("error", &colors),
        )
        .description("settings.urgent_color.description")
        .visible_when("use_urgency_colors", &["true"]),
        Setting::select(
            "idle_color",
            "settings.idle_color.label",
            ("primary", &colors),
        )
        .description("settings.idle_color.description"),
        Setting::int(
            "limit",
            "settings.limit.label",
            (10.0, Some(1.0), Some(60.0)),
        ),
        Setting::string_list("labels", "settings.labels.label", &["a", "b"]),
        Setting::glyph("glyph", "settings.glyph.label", "clock"),
    ]
}

fn build_plugin() -> Plugin {
    let meta = MetaBuilder::new()
        .id("iblameyou/example")
        .name("Example")
        .version("0.1.0")
        .author("IBlameYou")
        .api(57)
        .description("Neutral Noctalia plugin example")
        .tags(&["service", "widget", "panel", "launcher", "demo"])
        .icon("puzzle")
        .build();

    let mut plugin = Plugin::new(meta)
        .entry(service::update_entry())
        .entry(Entry::new(
            ("widget".to_owned(), EntryKind::Widget, settings()),
            widget::update,
        ))
        .entry(launcher::update_entry())
        .entry(panel::update_entry())
        .entry(shortcut::update_entry())
        .entry(desktop::update_entry())
        .entry(layer::update_entry());

    for (key, value) in TRANSLATIONS {
        plugin = plugin.tr(key, value);
    }

    plugin
}

#[must_use = "the exit code must be returned to the caller"]
pub fn run() -> ExitCode {
    run_cli(&build_plugin())
}

fn main() -> ExitCode {
    run()
}

#[cfg(test)]
mod tests {
    use super::*;
    use noctalia::{Parts, Value};
    use std::path::PathBuf;

    fn context() -> Context {
        Context::new(
            Parts {
                entry: "service".to_owned(),
                event: "update".to_owned(),
                payload: Value::Null,
                config: json!({}),
                state: json!({ "timers": [
                    {
                        "id": "one",
                        "seconds": 600,
                        "label": "10m",
                        "message": null,
                        "paused": false,
                        "endsAt": 601_000
                    }
                ]}),
                host: json!({ "now_ms": 1_000 }),
                data_dir: PathBuf::from("/tmp"),
            },
            json!({}),
        )
    }

    #[test]
    fn test_publish_and_timers_from_state() {
        let context = context();
        let timers = timers_from_state(&context);

        assert_eq!(timers.len(), 1);
        let value = serde_json::to_value(publish(&Store::default()))
            .expect("serialize");
        assert!(value["value"].get("timers").is_some());
    }

    #[test]
    fn test_timer_mutation_helpers() {
        let context = context();
        let mut store = Store::default();

        assert!(add_timer(&context, (&mut store, 300, None)));
        assert!(!add_timer(&context, (&mut store, 0, None)));
        assert_eq!(store.timers.len(), 1);

        let id = store.timers[0].id.clone();
        assert!(pause_by_id(&context, &mut store, &id));
        assert!(resume_by_id(&context, &mut store, &id));
        assert!(stop_by_id(&mut store, &id));
    }

    #[test]
    fn test_settings_and_build_plugin_cover_all_entries() {
        let settings = settings();
        let plugin = build_plugin();

        assert!(settings.len() > 10);
        assert_eq!(plugin.entries.len(), 7);
        assert_eq!(plugin.meta.id, "iblameyou/example");
    }
}
