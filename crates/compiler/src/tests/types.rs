use super::*;

#[test]
fn unwraps_bools_without_replacing_false() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let on = ctx.state_value::<bool>("on").unwrap_or(true);
    Ok(vec![Op::BarVisible { visible: on }])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Widget))
            .expect("compiles");
    assert!(
        compiled
            .source
            .contains("bool_or(plugin_state[\"on\"], true)"),
        "{}",
        compiled.source
    );
    assert!(
        compiled
            .source
            .contains("local function bool_or(value, default)")
    );
    assert!(!compiled.source.contains("(plugin_state[\"on\"] or true)"));
}

#[test]
fn compiles_exact_integer_and_float_bounds() {
    let bounds = [
        ("i8::MAX", "127"),
        ("i8::MIN", "(-128)"),
        ("i16::MAX", "32767"),
        ("i16::MIN", "(-32768)"),
        ("i32::MAX", "2147483647"),
        ("i32::MIN", "(-2147483648)"),
        ("i64::MAX", "9223372036854775807"),
        ("i64::MIN", "(-9223372036854775808)"),
        ("i128::MAX", "170141183460469231731687303715884105727"),
        ("u8::MAX", "255"),
        ("u16::MAX", "65535"),
        ("u32::MAX", "4294967295"),
        ("u64::MAX", "18446744073709551615"),
        ("u128::MAX", "340282366920938463463374607431768211455"),
        ("usize::MAX", "18446744073709551615"),
        ("isize::MAX", "9223372036854775807"),
        ("u8::MIN", "0"),
        ("usize::MIN", "0"),
        ("f64::MAX", "1.7976931348623157e308"),
        ("f64::MIN", "(-1.7976931348623157e308)"),
        ("f32::MAX", "3.4028234663852886e38"),
        ("f32::MIN", "(-3.4028234663852886e38)"),
    ];
    for (path, expected) in bounds {
        let handler = format!(
            r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {{
    Ok(vec![Op::log(format!("{{}}", {path}))])
}}
"#
        );
        let compiled =
            compile(&source(&handler), &options("handler", EntryKind::Service))
                .expect("compiles");
        assert!(
            compiled.source.contains(expected),
            "{path} should emit {expected}: {}",
            compiled.source
        );
        assert!(
            !compiled.source.contains("math.huge"),
            "{path}: {}",
            compiled.source
        );
    }
}

#[test]
fn compiles_typed_defaults_from_the_type_hint() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let name: String = Default::default();
    let count: u32 = Default::default();
    let flag: bool = Default::default();
    let items: Vec<Value> = Default::default();
    Ok(vec![Op::log(format!("{name}{count}{flag}{items}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let emitted = &compiled.source;
    assert!(emitted.contains("local name = \"\""), "{emitted}");
    assert!(emitted.contains("local count = 0"), "{emitted}");
    assert!(emitted.contains("local flag = false"), "{emitted}");
    assert!(emitted.contains("local items = {}"), "{emitted}");

    let untyped = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let unknown = Default::default();
    Ok(vec![Op::log(format!("{unknown}"))])
}
"#;
    let error =
        compile(&source(untyped), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(
        error.message.contains("needs a known type annotation"),
        "{}",
        error.message
    );
    assert!(error.line > 0 && error.column > 0, "{error:?}");
}
