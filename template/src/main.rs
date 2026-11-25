use std::process::ExitCode;

use noctalia_rs::{Context, Op, Result};

include!(concat!(env!("OUT_DIR"), "/generated.rs"));

// A function marked `#[native]` stays in this binary and runs through the
// helper process. Arguments and the return value cross as JSON, so take owned
// values and return something `serde` can serialize.
//
// #[native(cache_ms = 30_000)]
// fn normalize(text: String) -> String {
//     text.trim().to_lowercase()
// }
//
// Call it from a handler like any other function. The compiled Luau parks the
// handler while the helper computes, then continues with the result:
//
// fn widget(ctx: &Context) -> Result<Vec<Op>> {
//     let message = normalize(ctx.config_str("message").unwrap_or_default().into_owned());
//     Ok(vec![Op::BarText { text: message }])
// }
//
// Use `#[luau]` on a function that must never cross the boundary. The build
// then fails with `file:line:column` instead of splitting it out.

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
