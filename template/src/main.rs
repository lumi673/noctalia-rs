use std::process::ExitCode;

use noctalia_rs::{Context, Op, Result};

include!(concat!(env!("OUT_DIR"), "/generated.rs"));

#[expect(clippy::unnecessary_wraps, reason = "handlers return Result")]
fn service(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "init" || ctx.event == "update" {
        let message =
            ctx.config_str("message").unwrap_or_default().into_owned();
        Ok(vec![Op::set_interval(1000), Op::log(message)])
    } else {
        Ok(Vec::new())
    }
}

#[expect(clippy::unnecessary_wraps, reason = "handlers return Result")]
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "render" || ctx.event == "click" {
        let message =
            ctx.config_str("message").unwrap_or_default().into_owned();
        Ok(vec![Op::BarText { text: message }])
    } else {
        Ok(Vec::new())
    }
}

fn main() -> ExitCode {
    noctalia_rs::run(&plugin())
}
