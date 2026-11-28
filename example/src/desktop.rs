use noctalia_rs::entry;
use noctalia_rs::{Context, Op, Result, ui};
use serde_json::json;

#[entry(kind = "desktop_widget", id = "desktop")]
pub fn update(ctx: &Context) -> Result<Vec<Op>> {
    let _ = serde_json::to_value(&ctx.entry)?;
    let tree = ui::column(
        json!({ "gap": 8, "align": "center" }),
        vec![ui::label(json!({
            "text": "Example desktop widget",
            "fontWeight": "bold"
        }))],
    );

    Ok(vec![Op::DesktopWidgetRender { tree }])
}

#[cfg(test)]
mod tests {
    use super::*;
    use noctalia_rs::Value;
    use noctalia_rs::compat::Parts;
    use std::path::PathBuf;

    #[test]
    fn test_update_renders_desktop_widget() {
        let context = Context::new(
            Parts {
                entry: "desktop".to_owned(),
                event: "render".to_owned(),
                payload: Value::Null,
                config: Value::Null,
                state: Value::Null,
                host: Value::Null,
                data_dir: PathBuf::from("/tmp"),
            },
            Value::Null,
        );

        let ops = update(&context).expect("ops");

        assert_eq!(ops.len(), 1);
    }
}
