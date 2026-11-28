use super::ast::{ExprKind, Script, StmtKind};
use super::{
    fold_constants, parse_script, print_script, prune_callbacks,
    remove_dead_code, specialize_tables,
};

fn script(source: &str) -> Script {
    parse_script(source, "generated.lua")
}

#[test]
fn parser_printer_round_trip() {
    let source = concat!(
        "--!nonstrict\n",
        "-- Generated from Rust by noctalia-rs.\n",
        "\n",
        "local STATE_KEY = \"state\"\n",
        "local payload = { source = \"state\", nested = { count = 2, flag = true } }\n",
        "local function irem(a, b)\n",
        "  local r = a % b\n",
        "  if r ~= 0 then r = r + b end\n",
        "  return r\n",
        "end\n",
        "\n",
        "state.watch(STATE_KEY, function() emit(payload) end)\n",
    );
    assert_eq!(print_script(&script(source)), source);
}

#[test]
fn constant_folding_rejects_inexact_values() {
    let source = concat!(
        "local overflow = 9223372036854775807 + 1\n",
        "local edge = -9223372036854775807 - 1\n",
        "local inexact = 9007199254740993 + 1\n",
        "local exact = 9007199254740992 + 0\n",
        "local text = \"a\" .. \"b\"\n",
    );
    let folded = fold_constants(&script(source));
    let printed = print_script(&folded);
    assert!(printed.contains("local overflow = 9223372036854775807 + 1"));
    assert!(printed.contains("local edge = -9223372036854775807 - 1"));
    assert!(printed.contains("local inexact = 9007199254740993 + 1"));
    assert!(printed.contains("local exact = 9007199254740992"));
    assert!(printed.contains("local text = \"ab\""));
    let again = print_script(&fold_constants(&folded));
    assert_eq!(again, printed);
}

#[test]
fn ast_surface_round_trips_and_repasses() {
    let source = concat!(
        "local a = 1\n",
        "local t = { [1] = 2, a = 3, 4 }\n",
        "local x = t[1]\n",
        "local y = t.a\n",
        "local z = -x + #t\n",
        "local c = f(y, ...) or g()\n",
        "local h = function(q) return q end\n",
        "do\n",
        "  local b = 2\n",
        "  print(b)\n",
        "end\n",
        "while a < 3 do\n",
        "  a = a + 1\n",
        "end\n",
        "repeat\n",
        "  a = a - 1\n",
        "until a == 0\n",
        "for i = 1, 3, 1 do\n",
        "  print(i)\n",
        "end\n",
        "for k, v in pairs(t) do\n",
        "  print(k, v)\n",
        "end\n",
        "if a then\n",
        "  print(\"a\")\n",
        "elseif b then\n",
        "  print(\"b\")\n",
        "else\n",
        "  print(\"c\")\n",
        "end\n",
        "local function local_fn(q)\n",
        "  return q\n",
        "end\n",
        "function global_fn()\n",
        "  local_fn(a)\n",
        "end\n",
        "return a\n",
    );
    let parsed = script(source);
    let printed = print_script(&parsed);
    assert!(printed.contains("while"), "{printed}");
    assert!(printed.contains("repeat"), "{printed}");
    assert!(printed.contains("elseif"), "{printed}");
    assert!(printed.contains("function global_fn()"), "{printed}");
    let once = prune_callbacks(&remove_dead_code(&fold_constants(
        &specialize_tables(&parsed),
    )));
    let printed_once = print_script(&once);
    let twice = prune_callbacks(&remove_dead_code(&fold_constants(
        &specialize_tables(&once),
    )));
    assert_eq!(print_script(&twice), printed_once);
}

#[test]
fn constant_if_keeps_binding_scope() {
    let source = concat!(
        "local x = 0\n",
        "if true then\n",
        "  local x = 1\n",
        "end\n",
        "local x = x + 1\n",
        "print(x)\n",
    );
    let printed = print_script(&fold_constants(&script(source)));
    assert!(printed.contains("do"), "{printed}");
    assert!(printed.contains("local x = 1"), "{printed}");
    assert!(printed.contains("local x = x + 1"), "{printed}");
}

#[test]
fn folded_string_escapes_are_terminated() {
    let folded = fold_constants(&script("local text = \"\\1\" .. \"2\"\n"));
    let printed = print_script(&folded);
    assert!(printed.contains("\"\\0012\""), "{printed}");
}
#[test]
fn dead_code_removal_keeps_evaluation_order() {
    let source = concat!(
        "local used = 1\n",
        "local dead = 2\n",
        "host_call(used)\n",
        "return used\n",
        "host_call(\"never\")\n",
    );
    let cleaned = remove_dead_code(&script(source));
    let printed = print_script(&cleaned);
    assert!(printed.contains("local used = 1"));
    assert!(!printed.contains("local dead"));
    assert!(printed.contains("host_call(used)\nreturn used\n"));
    assert!(!printed.contains("never"));
    assert_eq!(print_script(&remove_dead_code(&cleaned)), printed);
}

#[test]
fn callback_pruning_keeps_official_callbacks() {
    let source = concat!(
        "local function body_x() end\n",
        "function onX() body_x() end\n",
        "function onY() body_x() end\n",
        "function __strict_callback(id, ...) body_x() end\n",
        "local callbacks = { cb = onX }\n",
    );
    let pruned = prune_callbacks(&script(source));
    let printed = print_script(&pruned);
    assert!(printed.contains("function onX() body_x() end"));
    assert!(printed.contains("function onY() body_x() end"));
    assert!(!printed.contains("function __strict_callback("));
    assert!(printed.contains("cb = onX"));
    assert_eq!(print_script(&prune_callbacks(&pruned)), printed);
}

#[test]
fn callback_pruning_keeps_multi_value_initializers() {
    let source =
        concat!("local cb, out = function() end, side()\n", "print(out)\n",);
    let printed = print_script(&prune_callbacks(&script(source)));
    assert!(printed.contains("side()"), "{printed}");
}

#[test]
fn table_specialization_leaves_object_tables_alone() {
    let source = concat!(
        "local config = { name = \"render\", count = 1 }\n",
        "host_call(config)\n",
    );
    let specialized = specialize_tables(&script(source));
    assert_eq!(print_script(&specialized), source);
}

#[test]
fn array_tables_get_marker_and_repeated_literals_are_hoisted() {
    let source = concat!(
        "host_call({ 1, 2 }, \"first\")\n",
        "host_call({ 1, 2 }, \"second\")\n",
    );
    let specialized = specialize_tables(&script(source));
    let printed = print_script(&specialized);
    assert!(printed.contains("local __noctalia_table_0 ="));
    assert!(printed.contains("__noctalia_table_0, \"first\""));
    assert!(printed.contains("__noctalia_array = true"));
    assert_eq!(print_script(&specialize_tables(&specialized)), printed);
}

#[test]
fn table_hoisting_distinguishes_field_boundaries() {
    let source = concat!(
        "host_call({ 1, 2 }, \"first\")\n",
        "host_call({ 12 }, \"second\")\n",
    );
    let printed = print_script(&specialize_tables(&script(source)));
    assert!(!printed.contains("__noctalia_table_"), "{printed}");
    assert!(printed.contains("{ 1, 2}"), "{printed}");
    assert!(printed.contains("{ 12}"), "{printed}");
}

#[test]
fn proven_is_array_call_is_folded_and_helper_removed() {
    let source = concat!(
        "local function is_array(value) return value ~= nil end\n",
        "local values = setmetatable({ 1, 2 }, { __noctalia_array = true })\n",
        "local flag = is_array(values)\n",
        "host_call(flag)\n",
    );
    let specialized = specialize_tables(&script(source));
    let printed = print_script(&specialized);
    assert!(printed.contains("local flag = true"));
    assert!(!printed.contains("local function is_array"));
}

#[test]
fn mutated_is_array_binding_is_not_folded() {
    let source = concat!(
        "local function is_array(value) return value ~= nil end\n",
        "local values = setmetatable({ 1, 2 }, { __noctalia_array = true })\n",
        "values = { 3 }\n",
        "local flag = is_array(values)\n",
    );
    let specialized = specialize_tables(&script(source));
    let printed = print_script(&specialized);
    assert!(printed.contains("is_array(values)"));
    assert!(printed.contains("local function is_array"));
}

#[test]
fn span_renders_as_file_line_column() {
    let parsed = script("local x = 1\n");
    assert!(parsed.statements.first().is_some_and(|statement| {
        statement.span.to_string() == "generated.lua:1:1"
    }));
    assert!(parsed.statements.first().is_some_and(|statement| {
        let StmtKind::Local { values, .. } = &statement.kind else {
            return false;
        };
        values
            .first()
            .is_some_and(|value| value.span.to_string() == "generated.lua:1:11")
    }));
}

#[test]
fn parses_realistic_generated_strict_script() {
    let source = concat!(
        "--!nonstrict\n",
        "-- Generated from Rust by noctalia-rs.\n",
        "\n",
        "local STATE_KEY = \"state\"\n",
        "local ENTRY = \"render\"\n",
        "local PAYLOAD_EMPTY = {}\n",
        "local CONFIG = {}\n",
        "\n",
        "local function body_render(...)\n",
        "  local args = { name = \"render\", args = { ... } }\n",
        "  local callback = function(value) return value end\n",
        "  if args.name == \"render\" then callback(args) end\n",
        "  return callback(args)\n",
        "end\n",
        "\n",
        "function onRender(...)\n",
        "  return body_render(...)\n",
        "end\n",
        "\n",
        "local url = \"https://example.test/a--b\"\n",
        "host_call(url)\n",
        "state.watch(STATE_KEY, function()\n",
        "  local payload = { source = \"state\", nested = { flag = true, count = 2 } }\n",
        "  body_render(payload)\n",
        "end)\n",
    );
    let parsed = script(source);
    assert_eq!(print_script(&parsed), source);
    assert!(parsed.statements.iter().any(|statement| matches!(
        statement.kind,
        StmtKind::LocalFunction { .. }
    )));
    assert!(
        parsed.statements.iter().any(|statement| matches!(
            statement.kind,
            StmtKind::Function { .. }
        ))
    );
    assert!(parsed.statements.iter().any(|statement| {
        let StmtKind::Call(expr) = &statement.kind else {
            return false;
        };
        matches!(expr.kind, ExprKind::Call { ref callee, .. }
            if matches!(callee.kind, ExprKind::Field { .. }))
    }));
    assert!(parsed.statements.iter().any(|statement| {
        let StmtKind::Local { values, .. } = &statement.kind else {
            return false;
        };
        values
            .first()
            .is_some_and(|value| matches!(value.kind, ExprKind::String(ref text) if text == "https://example.test/a--b"))
    }));
}

#[test]
fn typed_ir_lowers_to_printable_luau_ast() {
    let source = concat!(
        "use noctalia_rs::{Context, Op, Result};\n",
        "fn handler(_ctx: &Context) -> Result<Vec<Op>> {\n",
        "  let value = 1i64 + 2;\n",
        "  if value > 2 {\n",
        "    return Ok(Vec::new());\n",
        "  }\n",
        "  Ok(Vec::new())\n",
        "}\n",
    );
    let options = crate::Options {
        handler: "handler",
        entry: "strict",
        state_key: "strict.state",
        kind: crate::EntryKind::Service,
        strings: &[],
        settings: &[],
        module: "main",
        modules: &[],
        native: crate::NativeMode::Deny,
        strict: None,
    };
    let program =
        crate::ir::lower_strict(source, &options).expect("lower typed IR");
    let script = super::lower_ir(&program).expect("lower Luau AST");
    let printed = print_script(&script);
    assert!(printed.contains("function handler("), "{printed}");
    assert!(printed.contains(" = 1 + 2"), "{printed}");
    assert!(printed.contains("if value_b"), "{printed}");
    assert!(printed.contains("> 2 then"), "{printed}");
}

#[test]
fn parenthesized_constants_fold() {
    let folded = fold_constants(&script("local value = (20 + 22)\n"));
    assert_eq!(print_script(&folded), "local value = 42\n");
}
