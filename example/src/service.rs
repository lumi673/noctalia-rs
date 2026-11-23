use noctalia_rs::entry;
use noctalia_rs::{Context, Op, Result};

use super::duration;
use super::timer;
use super::{add_timer, pause_by_id, publish, resume_by_id, stop_by_id};

#[entry(kind = "service", id = "service")]
pub fn update(ctx: &Context) -> Result<Vec<Op>> {
    let mut store = timer::load(ctx);
    let mut ops = Vec::new();

    match ctx.event.as_str() {
        "init" | "update" | "resume" => expire(ctx, &mut store, &mut ops),
        "ipc" => ipc(ctx, &mut store, &mut ops),
        _ => {}
    }

    timer::save(ctx, &store)?;
    ops.push(Op::SetInterval {
        ms: timer::next_interval_ms(&store, ctx.now_ms),
    });
    ops.push(publish(&store));

    Ok(ops)
}

fn expire(ctx: &Context, store: &mut timer::Store, ops: &mut Vec<Op>) {
    let before = store.timers.len();

    store.timers.retain(|timer| {
        if timer.paused || timer.ends_at.is_some_and(|end| end > ctx.now_ms) {
            return true;
        }

        let body = timer.message.clone().unwrap_or_else(|| {
            ctx.tr("notify.body", &[("duration", timer.label.clone())])
        });
        ops.push(Op::Notify {
            title: ctx.tr("notify.title", &[]),
            body,
            options: None,
        });

        false
    });

    let finished = before.saturating_sub(store.timers.len());
    if finished > 0 {
        ops.push(Op::log(format!("example: {finished} finished")));
    }
}

fn ipc(ctx: &Context, store: &mut timer::Store, ops: &mut Vec<Op>) {
    let event = ctx.payload_str("event").unwrap_or_default();
    let payload = ctx.payload_str("payload").unwrap_or_default();

    match event.as_str() {
        "start" => {
            if let Some((seconds, message)) = duration::split(&payload) {
                let _ = add_timer(ctx, (store, seconds, message));
            }
        }
        "stop" => {
            let _ = stop_by_id(store, &payload);
        }
        "pause" => {
            let _ = pause_by_id(ctx, store, &payload);
        }
        "resume" => {
            let _ = resume_by_id(ctx, store, &payload);
        }
        "stop-all" => store.timers.clear(),
        "list" => list(ctx, store, ops),
        _ => {}
    }
}

fn list(ctx: &Context, store: &timer::Store, ops: &mut Vec<Op>) {
    for timer in &store.timers {
        let seconds = timer.remaining_ms(ctx.now_ms).div_ceil(1_000);
        let state = if timer.paused { "paused" } else { "running" };
        let message = format!(
            "example: {} {} {} left",
            timer.id,
            state,
            duration::clock(seconds)
        );
        ops.push(Op::log(message));
    }
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

    fn context(event: &str, payload: Value, data_dir: PathBuf) -> Context {
        Context::new(
            Parts {
                entry: "service".to_owned(),
                event: event.to_owned(),
                payload,
                config: json!({}),
                state: json!({}),
                host: json!({ "now_ms": 1_000 }),
                data_dir,
            },
            json!({ "notify": { "title": "Example", "body": "{duration} done" } }),
        )
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            env::temp_dir().join(format!("service-{name}-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn test_update_expires_finished_timer() {
        let dir = temp_dir("expired");
        let context = context("update", json!({}), dir.clone());
        let store = timer::Store {
            timers: vec![timer::Timer {
                id: "old".to_owned(),
                seconds: 60,
                label: "1m".to_owned(),
                message: None,
                paused: false,
                ends_at: Some(500),
                remaining_ms: None,
            }],
            ..timer::Store::default()
        };
        timer::save(&context, &store).expect("save");

        let ops = update(&context).expect("ops");

        assert!(ops.iter().any(|op| matches!(op, Op::Notify { .. })));
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn test_ipc_controls_timers() {
        let dir = temp_dir("ipc");
        let start_context = context(
            "ipc",
            json!({ "event": "start", "payload": "5m" }),
            dir.clone(),
        );

        update(&start_context).expect("start");
        let store = timer::load(&start_context);
        let id = store.timers[0].id.clone();

        for event in ["stop", "pause", "resume", "list"] {
            let ipc_context = context(
                "ipc",
                json!({ "event": event, "payload": id.clone() }),
                dir.clone(),
            );
            update(&ipc_context).expect(event);
        }

        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn test_update_without_timers_uses_long_interval() {
        let dir = temp_dir("interval");
        let context = context("init", json!({}), dir.clone());

        let ops = update(&context).expect("ops");

        assert!(
            ops.iter()
                .any(|op| matches!(op, Op::SetInterval { ms: 86_400_000 }))
        );
        fs::remove_dir_all(dir).expect("cleanup");
    }
}
