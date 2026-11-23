use noctalia_rs::entry;
use noctalia_rs::{Context, Op, Result};
use serde_json::json;

#[entry(kind = "shortcut", id = "shortcut")]
pub fn update(ctx: &Context) -> Result<Vec<Op>> {
    let _ = serde_json::to_value(&ctx.entry)?;
    if ctx.event != "click" {
        return Ok(Vec::new());
    }

    let enabled = ctx
        .state
        .get("enabled")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or_default();

    Ok(vec![Op::state(
        "example.enabled",
        json!({ "enabled": !enabled }),
    )])
}

#[cfg(test)]
mod tests {
    use super::*;
    use noctalia_rs::{Parts, Value};
    use std::path::PathBuf;

    fn context(event: &str, enabled: bool) -> Context {
        Context::new(
            Parts {
                entry: "shortcut".to_owned(),
                event: event.to_owned(),
                payload: Value::Null,
                config: Value::Null,
                state: json!({ "enabled": enabled }),
                host: Value::Null,
                data_dir: PathBuf::from("/tmp"),
            },
            Value::Null,
        )
    }

    #[test]
    fn test_update_toggles_enabled_on_click() {
        let context = context("click", false);

        let ops = update(&context).expect("ops");

        assert_eq!(ops.len(), 1);
    }

    #[test]
    fn test_update_ignores_render_event() {
        let context = context("render", false);

        let ops = update(&context).expect("ops");

        assert!(ops.is_empty());
    }
}
