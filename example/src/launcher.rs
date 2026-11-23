use noctalia_rs::entry;
use noctalia_rs::{Context, Op, Result};
use serde_json::{Value, json};

use super::duration;
use super::timer;
use super::{
    pause_by_id, publish, resume_by_id, stop_by_id, timers_from_state,
};

type ActionRows<'a> = (&'a str, &'a [timer::Timer], i64);
type TimerRow<'a> = (&'a str, &'a timer::Timer, i64);

#[entry(kind = "launcher", id = "launcher")]
pub fn update(ctx: &Context) -> Result<Vec<Op>> {
    match ctx.event.as_str() {
        "query" => {
            let text = ctx.payload_str("text").unwrap_or_default();
            let rows = query_rows(ctx, &text);

            Ok(vec![Op::LauncherResults { text, rows }])
        }
        "activate" => activate(ctx),
        _ => Ok(Vec::new()),
    }
}

fn activate(ctx: &Context) -> Result<Vec<Op>> {
    let id = ctx.payload_str("id").unwrap_or_default();
    let mut store = timer::load(ctx);

    if id == "stop-all" {
        store.timers.clear();
    } else if let Some((action, rest)) = id.split_once(':') {
        match action {
            "pause" => {
                let _ = pause_by_id(ctx, &mut store, rest);
            }
            "resume" => {
                let _ = resume_by_id(ctx, &mut store, rest);
            }
            "stop" => {
                let _ = stop_by_id(&mut store, rest);
            }
            "start" => start(&mut store, ctx.now_ms, rest),
            _ => {}
        }
    }

    timer::save(ctx, &store)?;

    Ok(vec![publish(&store)])
}

fn start(store: &mut timer::Store, now: i64, rest: &str) {
    let Some((seconds_text, message)) = rest.split_once(':') else {
        return;
    };
    let Ok(seconds) = seconds_text.parse::<u64>() else {
        return;
    };
    if seconds == 0 {
        return;
    }

    store.seq = store.seq.saturating_add(1);
    let offset = i64::try_from(seconds)
        .unwrap_or(i64::MAX)
        .saturating_mul(1_000);
    let text = if message.is_empty() {
        None
    } else {
        Some(message.to_owned())
    };

    store.timers.push(timer::Timer {
        id: format!("{now}-{}", store.seq),
        seconds,
        label: duration::human(seconds),
        message: text,
        paused: false,
        ends_at: Some(now.saturating_add(offset)),
        remaining_ms: None,
    });
    timer::sort(store);
}

fn query_rows(ctx: &Context, text: &str) -> Vec<Value> {
    let query = text.trim();
    let timers = timers_from_state(ctx);
    let now = ctx.now_ms;

    match query {
        "" => vec![row((
            "new",
            ctx.tr("launcher.start_new", &[]),
            ctx.tr("launcher.hint", &[]),
            "plus",
        ))],
        "pause" | "resume" | "stop" => action_rows(ctx, (query, &timers, now)),
        "stop all" => stop_all_rows(ctx, &timers),
        _ => duration_row(ctx, query),
    }
}

fn action_rows(ctx: &Context, spec: ActionRows<'_>) -> Vec<Value> {
    let (action, timers, now) = spec;
    let want_paused = action == "resume";
    let mut rows = timers
        .iter()
        .filter(|timer| action == "stop" || timer.paused == want_paused)
        .map(|timer| timer_row(ctx, (action, timer, now)))
        .collect::<Vec<_>>();

    if rows.is_empty() {
        rows.push(empty_row(ctx));
    }

    rows
}

fn stop_all_rows(ctx: &Context, timers: &[timer::Timer]) -> Vec<Value> {
    if timers.is_empty() {
        return vec![empty_row(ctx)];
    }

    vec![row((
        "stop-all",
        ctx.tr("launcher.stop_all", &[]),
        ctx.tr(
            "launcher.active_count",
            &[("count", timers.len().to_string())],
        ),
        "player-stop",
    ))]
}

fn duration_row(ctx: &Context, query: &str) -> Vec<Value> {
    let Some((seconds, message)) = duration::split(query) else {
        return vec![row((
            "invalid",
            ctx.tr("launcher.invalid", &[]),
            ctx.tr("launcher.hint", &[]),
            "alert-triangle",
        ))];
    };

    let human = duration::human(seconds);
    let subtitle = message.as_ref().map_or_else(
        || human.clone(),
        |text| {
            ctx.tr(
                "launcher.message",
                &[("duration", human.clone()), ("message", text.clone())],
            )
        },
    );
    let id = message.map_or_else(
        || format!("start:{seconds}:"),
        |text| format!("start:{seconds}:{text}"),
    );

    vec![row((
        &id,
        ctx.tr("launcher.start", &[("time", human)]),
        subtitle,
        "clock",
    ))]
}

fn timer_row(ctx: &Context, spec: TimerRow<'_>) -> Value {
    let (action, timer, now) = spec;
    let title = timer.message.clone().unwrap_or_else(|| {
        ctx.tr("launcher.timer", &[("duration", timer.label.clone())])
    });
    let left = duration::approx(timer.remaining_ms(now).div_ceil(1_000));
    let subtitle = if timer.paused {
        ctx.tr("launcher.paused_at", &[("duration", left)])
    } else {
        ctx.tr("launcher.ends_at", &[("duration", left)])
    };

    row((&format!("{action}:{}", timer.id), title, subtitle, action))
}

fn empty_row(ctx: &Context) -> Value {
    row((
        "empty",
        ctx.tr("launcher.no_matching", &[]),
        ctx.tr("launcher.hint", &[]),
        "clock",
    ))
}

fn row(parts: (&str, String, String, &str)) -> Value {
    let (id, title, subtitle, glyph) = parts;
    json!({ "id": id, "title": title, "subtitle": subtitle, "glyph": glyph })
}

#[cfg(test)]
mod tests {
    use super::*;
    use noctalia_rs::{Parts, Value};
    use serde_json::json;
    use std::env;
    use std::fs;
    use std::path::PathBuf;
    use std::process;

    fn translations() -> Value {
        json!({
            "launcher": {
                "start_new": "Start",
                "hint": "Hint",
                "no_matching": "Empty",
                "stop_all": "Stop all",
                "active_count": "{count} timers",
                "invalid": "Invalid",
                "start": "Start {time}",
                "message": "{duration} - {message}",
                "timer": "{duration} timer",
                "paused_at": "Paused {duration}",
                "ends_at": "Ends {duration}"
            }
        })
    }

    fn query_context(text: &str) -> Context {
        Context::new(
            Parts {
                entry: "launcher".to_owned(),
                event: "query".to_owned(),
                payload: json!({ "text": text }),
                config: json!({}),
                state: json!({ "timers": [
                    {
                        "id": "one",
                        "seconds": 600,
                        "label": "10m",
                        "message": null,
                        "paused": false,
                        "endsAt": 601_000
                    },
                    {
                        "id": "two",
                        "seconds": 300,
                        "label": "5m",
                        "message": null,
                        "paused": true,
                        "remainingMs": 120_000
                    }
                ]}),
                host: json!({ "now_ms": 1_000 }),
                data_dir: PathBuf::from("/tmp"),
            },
            translations(),
        )
    }

    fn activate_context(id: &str, name: &str) -> Context {
        let dir =
            env::temp_dir().join(format!("launcher-{name}-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");

        Context::new(
            Parts {
                entry: "launcher".to_owned(),
                event: "activate".to_owned(),
                payload: json!({ "id": id }),
                config: json!({}),
                state: json!({}),
                host: json!({ "now_ms": 1_000 }),
                data_dir: dir,
            },
            translations(),
        )
    }

    #[test]
    fn test_query_branches_produce_rows() {
        for text in ["", "pause", "resume", "stop", "stop all", "5m", "bad"] {
            let context = query_context(text);

            let ops = update(&context).expect("ops");

            assert!(matches!(ops[0], Op::LauncherResults { .. }), "{text}");
        }
    }

    #[test]
    fn test_activate_handles_all_actions() {
        let context = activate_context("start:300:hello", "start");
        update(&context).expect("start");

        for (id, name) in [
            ("pause:one", "pause"),
            ("resume:one", "resume"),
            ("stop:one", "stop"),
            ("stop-all", "stopall"),
        ] {
            let context = activate_context(id, name);
            update(&context).expect(name);
        }
    }
}
