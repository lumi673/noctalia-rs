use noctalia_rs::entry;
use noctalia_rs::{Context, LayerEvent, Op, Result, ui};
use serde_json::json;

#[entry(kind = "layer", id = "layer")]
pub fn update(ctx: &Context) -> Result<Vec<Op>> {
    let _ = serde_json::to_value(&ctx.entry)?;
    if ctx.layer_event() == LayerEvent::Click {
        return Ok(vec![Op::LayerHide {
            id: "layer".to_owned(),
        }]);
    }

    let tree = ui::row(
        json!({ "gap": 8, "align": "center" }),
        vec![ui::label(json!({ "text": "Example layer" }))],
    );

    Ok(vec![
        Op::LayerShow {
            id: "layer".to_owned(),
        },
        Op::LayerRender { tree },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use noctalia_rs::{Parts, Value};
    use std::path::PathBuf;

    fn context(event: &str) -> Context {
        Context::new(
            Parts {
                entry: "layer".to_owned(),
                event: event.to_owned(),
                payload: Value::Null,
                config: Value::Null,
                state: Value::Null,
                host: Value::Null,
                data_dir: PathBuf::from("/tmp"),
            },
            Value::Null,
        )
    }

    #[test]
    fn test_update_hides_layer_on_click() {
        let context = context("click");

        let ops = update(&context).expect("ops");

        assert_eq!(ops.len(), 1);
    }

    #[test]
    fn test_update_shows_and_renders_layer() {
        let context = context("render");

        let ops = update(&context).expect("ops");

        assert_eq!(ops.len(), 2);
    }
}
