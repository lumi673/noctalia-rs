use super::*;

#[test]
fn resolves_string_and_number_constants() {
    let handler = r#"
pub const STATE_KEY: &str = "example.state";
const RETRY_MS: u64 = 5000;

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let _ = ctx.now_ms;
    Ok(vec![
        Op::state(STATE_KEY, json!({ "retry": RETRY_MS })),
        Op::set_interval(RETRY_MS),
    ])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled
            .source
            .contains("state.set(\"example.state\", { retry = 5000 })")
    );
    assert!(compiled.source.contains("noctalia.setUpdateInterval(5000)"));
}

#[test]
fn hoists_constant_payload_tables_only_when_safe() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    match ctx.event.as_str() {
        "activate" => {
            let id = ctx.payload_str("id").unwrap_or_default();
            Ok(vec![Op::log(id)])
        }
        _ => Ok(Vec::new()),
    }
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("function update() end"));
    assert!(
        compiled
            .source
            .contains("function onActivate(id) body_activate(id) end")
    );
    assert!(
        compiled
            .source
            .contains("local function body_activate(arg_id)")
    );

    let unmapped = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    match ctx.event.as_str() {
        "activate" => {
            let value = ctx.payload_arg(0).unwrap_or(0);
            Ok(vec![Op::log(format!("{value}"))])
        }
        _ => Ok(Vec::new()),
    }
}
"#;
    let compiled =
        compile(&source(unmapped), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("local PAYLOAD_UPDATE"));
    assert!(compiled.source.contains("local PAYLOAD_LOAD"));
    assert!(compiled.source.contains("body_activate(payload)"));

    let escaping = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::state("payload", ctx.payload.clone())])
}
"#;
    let compiled =
        compile(&source(escaping), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(!compiled.source.contains("PAYLOAD_UPDATE"));
    assert!(compiled.source.contains(
        "function update() body_update({ source = \"update\" }) end"
    ));
}

#[test]
fn compiles_once_guards_and_once_locks() {
    let handler = r#"
use std::sync::{Once, OnceLock};
static VALUE: OnceLock<String> = OnceLock::new();
static ONCE: Once = Once::new();
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    let value = VALUE.get_or_init(|| "ready".to_owned());
    ONCE.call_once(|| ops.push(Op::log("once")));
    VALUE.set("later".to_owned());
    ops.push(Op::log(value));
    ops.push(Op::log(format!("{}", ctx.event.as_str())));
    Ok(ops)
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("local __static_main__VALUE"));
    assert!(
        compiled
            .source
            .contains("if __static_main__VALUE == nil then")
    );
    assert!(compiled.source.contains("if not __static_main__ONCE then"));
    assert!(compiled.source.contains("__static_main__ONCE = true"));
}

#[test]
fn covers_static_containers_and_atomic_operations() {
    let handler = r#"
use std::sync::{Once, OnceLock, Mutex, RwLock, atomic::{AtomicBool, AtomicU64, Ordering}};
static FLAG: Once = Once::new();
static CELL: OnceLock<String> = OnceLock::new();
static LOCK: Mutex<i64> = Mutex::new(1);
static RW: RwLock<i64> = RwLock::new(2);
static BOOL: AtomicBool = AtomicBool::new(false);
static COUNT: AtomicU64 = AtomicU64::new(0);
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    FLAG.call_once(|| ops.push(Op::log("once")));
    let value = CELL.get_or_init(|| "init".to_owned());
    CELL.set("set".to_owned());
    let _ = CELL.take();
    let _ = LOCK.lock().unwrap();
    let _ = RW.read().unwrap();
    let _ = RW.write().unwrap();
    BOOL.store(true, Ordering::SeqCst);
    let _ = BOOL.load(Ordering::SeqCst);
    let _ = COUNT.swap(1, Ordering::SeqCst);
    let _ = COUNT.fetch_add(1, Ordering::SeqCst);
    let _ = COUNT.fetch_sub(1, Ordering::SeqCst);
    ops.push(Op::log(value));
    Ok(ops)
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("__static_main__CELL"));
}

#[test]
fn covers_static_initializers_and_remaining_methods() {
    let handler = r#"
use std::cell::{Cell, RefCell};
use std::sync::{LazyLock, Mutex, atomic::{AtomicBool, Ordering}};
struct Config { enabled: bool, name: String }
static CONFIG: Mutex<Config> = Mutex::new(Config { enabled: true, name: "ready" });
static LAZY: LazyLock<String> = LazyLock::new(|| "lazy".to_owned());
static CELL: Cell<i64> = Cell::new(1);
static REF: RefCell<String> = RefCell::new("shared".to_owned());
static FLAG: AtomicBool = AtomicBool::new(true);
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let value = LAZY.deref();
    let cell = CELL.get();
    let shared = REF.borrow();
    let enabled = FLAG.load(Ordering::SeqCst);
    let config = CONFIG.lock().unwrap();
    let mut optional: Option<String> = None;
    let inserted = optional.get_or_insert("inserted".to_owned());
    let parsed = ctx.config_str("message").and_then(|text| text.parse::<f64>().ok());
    let expected = ctx.config_str("message").expect("missing");
    let map = std::collections::BTreeMap::new();
    let set = std::collections::HashSet::new();
    let list = std::collections::HashMap::new();
    Ok(vec![Op::log(format!("{}{}{}{}{}{}{}{}{}{}{}", value, cell, shared, enabled, config.enabled, inserted, parsed.unwrap_or(0.0), expected, map.len(), set.len(), list.len()))])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("local __static_main__CONFIG"));
}

#[test]
fn compiles_richer_constants_and_defaults() {
    let handler = r#"
const LIMIT: f64 = 380.0;
const FLOOR: f64 = -1.0;
const TIERS: [(&str, i64); 2] = [("a", 1), ("b", 2)];
struct State { items: Vec<String> }
impl State {
    const EMPTY: State = State { items: Vec::new() };
}
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut total = LIMIT + FLOOR;
    for (name, weight) in TIERS {
        total += weight as f64;
        let _ = name;
    }
    let state = State::EMPTY;
    let capacity = Vec::with_capacity(4);
    let biggest = std::cmp::max(1, 2);
    let ceiling = i64::MAX;
    let smallest = f64::MIN;
    let empty: Map<String, Value> = Map::default();
    Ok(vec![Op::log(format!("{total} {} {} {} {} {}", state.items, capacity.len(), biggest, ceiling, smallest, empty))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let source = &compiled.source;
    assert!(source.contains("380.0"), "{source}");
    assert!(source.contains("(-1.0)"), "{source}");
    assert!(source.contains("9223372036854775807"), "{source}");
    assert!(source.contains("math.max(1, 2)"), "{source}");
    assert!(source.contains("{}"), "{source}");
}
