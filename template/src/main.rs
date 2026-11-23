use std::process::ExitCode;

use noctalia::{Context, Op, Result};

include!(concat!(env!("OUT_DIR"), "/generated.rs"));

fn service(ctx: &Context) -> Result<Vec<Op>> {
    let _ = serde_json::from_str::<serde_json::Value>("null")?;
    if ctx.event == "init" || ctx.event == "update" {
        let message =
            ctx.config_str("message").unwrap_or_default().into_owned();
        Ok(vec![Op::set_interval(1000), Op::log(message)])
    } else {
        Ok(Vec::new())
    }
}

fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let _ = serde_json::from_str::<serde_json::Value>("null")?;
    if ctx.event == "render" || ctx.event == "click" {
        let message =
            ctx.config_str("message").unwrap_or_default().into_owned();
        Ok(vec![Op::BarText { text: message }])
    } else {
        Ok(Vec::new())
    }
}

fn main() -> ExitCode {
    noctalia::run(&plugin())
}
