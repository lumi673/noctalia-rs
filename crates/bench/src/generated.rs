//! Compiled-from-Rust Luau against hand-written Luau.
//!
//! Each fixture runs the same work three ways: the native Rust handler, the
//! Luau script `noctalia-rs-compiler` derives from that handler, and a
//! hand-written Luau script that mirrors the generated startup. Outputs are
//! compared before timing and must be identical.
//!
//! The target is parity: compiled Luau must run at least as fast as the
//! hand-written Luau it replaces. Both sides pay the same ~2 us `dispatch`
//! floor, so the 3x target in the report applies to the spool IPC path
//! (`spool_dispatch`), not to this comparison.

use serde::Serialize;
use serde_json::{Value, json};

use noctalia_rs::{Context, Op, Result as HandlerResult, ui};
use noctalia_rs_compiler::{EntryKind, Options as CompileOptions, compile};

use super::plugins::canonical;
use super::{Options, Pair, Result, Summary, context, measure};
use std::fs;
use std::path::Path;

const HELLO_BAR_SOURCE: &str = r#"
use noctalia_rs::{Context, Op, Result};

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let message = ctx.config_str("message").unwrap_or_default().into_owned();
    if ctx.event == "render" {
        Ok(vec![Op::BarText { text: message }])
    } else if ctx.event == "click" {
        let log = format!("click {message}");
        Ok(vec![Op::BarText { text: message }, Op::log(log)])
    } else {
        Ok(Vec::new())
    }
}
"#;

const HELLO_BAR_HANDWRITTEN: &str = r#"
local STATE_KEY = "hello.state"

local function render(event)
  local message = noctalia.getConfig("message") or ""
  if event == "render" then
    barWidget.setText(message)
  elseif event == "click" then
    local log = "click " .. message
    barWidget.setText(message)
    noctalia.log(log)
  end
end

function update() render("render") end
function onClick() render("click") end
noctalia.state.watch(STATE_KEY, function() render("render") end)
render("render")
"#;

const UI_PANEL_SOURCE: &str = r#"
use noctalia_rs::{Context, Op, Result, ui};
use serde_json::json;

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let label = ctx.state_value::<String>("label").unwrap_or_default();
    let tree = ui::column(
        json!({ "id": "root", "gap": 4 }),
        vec![
            ui::row(
                json!({ "id": "header" }),
                vec![
                    ui::label(json!({ "id": "title", "text": "Status" })),
                    ui::label(json!({ "id": "value", "text": label })),
                ],
            ),
            ui::button(json!({ "id": "save", "label": "Save" })),
            ui::input(json!({ "id": "filter", "placeholder": "Filter" })),
            ui::progress(json!({ "id": "progress", "value": 0.75 })),
        ],
    );
    Ok(vec![Op::PanelRender { tree }])
}
"#;

const UI_PANEL_HANDWRITTEN: &str = r#"
local STATE_KEY = "panel.state"

local function render()
  local store = noctalia.state.get(STATE_KEY) or {}
  local label = store["label"] or ""
  local tree = ui.column({ id = "root", gap = 4 }, {
    ui.row({ id = "header" }, {
      ui.label({ id = "title", text = "Status" }),
      ui.label({ id = "value", text = label })
    }),
    ui.button({ id = "save", label = "Save" }),
    ui.input({ id = "filter", placeholder = "Filter" }),
    ui.progress({ id = "progress", value = 0.75 })
  })
  panel.render(tree)
end

function update() render() end
noctalia.state.watch(STATE_KEY, render)
panel.setWantsSecondTicks(true)
render()
"#;

const EVENT_SOURCE: &str = r#"
use noctalia_rs::{Context, Op, Result};
use serde_json::json;

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    match ctx.event.as_str() {
        "activate" => {
            let id = ctx.payload_str("id").unwrap_or_default();
            let prefix = ctx.config_str("prefix").unwrap_or_default().into_owned();
            let status = format!("{prefix}:{id}");
            Ok(vec![
                Op::state("service.status", json!({ "id": id, "status": status.clone() })),
                Op::log(status.clone()),
                Op::BarText { text: status },
            ])
        }
        _ => Ok(Vec::new()),
    }
}
"#;

const EVENT_HANDWRITTEN: &str = r#"
local function render(event, payload)
  if event == "activate" then
    local id = payload["id"] or ""
    local prefix = noctalia.getConfig("prefix") or ""
    local status = prefix .. ":" .. id
    state.set("service.status", { id = id, status = status })
    noctalia.log(status)
    barWidget.setText(status)
  end
end

function update() render("update", {}) end
function onActivate(id) render("activate", { id = id }) end
render("load", {})
"#;

type Native = fn(&Context) -> HandlerResult<Vec<Op>>;

#[derive(Clone, Copy)]
enum Side {
    Compiled,
    Handwritten,
}

impl Side {
    const fn name(self) -> &'static str {
        match self {
            Self::Compiled => "compiled",
            Self::Handwritten => "handwritten",
        }
    }
}

struct Plan {
    workload: &'static str,
    state_key: &'static str,
    kind: EntryKind,
    compiled_entry: &'static str,
    handwritten_entry: &'static str,
}

impl Plan {
    /// Each side gets its own entry name so the global VM cache cannot serve
    /// one side's runtime to the other.
    fn new(
        workload: &'static str,
        kind: EntryKind,
        state_key: &'static str,
    ) -> Self {
        Self {
            workload,
            state_key,
            kind,
            compiled_entry: surface(workload, Side::Compiled),
            handwritten_entry: surface(workload, Side::Handwritten),
        }
    }

    fn entry(&self, side: Side) -> &'static str {
        match side {
            Side::Compiled => self.compiled_entry,
            Side::Handwritten => self.handwritten_entry,
        }
    }
}

/// On the two small fixtures the ~2 us dispatch floor dominates: the generated
/// entry shim costs one payload table per call and `tostring` on shorthand
/// `format!` captures, while hand-written Luau passes nothing. On the panel
/// render the generated node tables skip the `ui.*` constructor calls and win.
struct Fixture {
    plan: Plan,
    source: String,
    handwritten: &'static str,
    native: Native,
    payload: Value,
    /// Events compared before timing. The first one is the measured event.
    events: &'static [&'static str],
}

impl Fixture {
    fn workload(&self) -> &'static str {
        self.plan.workload
    }

    fn source(&self, side: Side) -> &str {
        match side {
            Side::Compiled => &self.source,
            Side::Handwritten => self.handwritten,
        }
    }
}

/// One fixture measured on one event in one scratch directory.
struct Run<'a> {
    fixture: &'a Fixture,
    event: &'a str,
    dir: &'a Path,
}

impl Run<'_> {
    /// The host delivers plugin state under the watched key, so a script reads
    /// it through `noctalia.state.get(STATE_KEY)`. A native handler receives
    /// that same object as `ctx.state` without the extra key.
    fn context(&self, side: Side) -> Context {
        let entry = self.fixture.plan.entry(side);
        let mut ctx = context(
            (entry, self.event),
            self.dir,
            self.fixture.payload.clone(),
        );
        let mut state = serde_json::Map::new();
        state.insert(self.fixture.plan.state_key.to_owned(), ctx.state.clone());
        ctx.state = Value::Object(state);
        ctx
    }

    fn ops(&self, side: Side) -> Result<Vec<Op>> {
        let sources =
            [(self.fixture.plan.entry(side), self.fixture.source(side))];
        let ctx = self.context(side);
        // The first dispatch builds the VM and replays the load render. The
        // second one returns only the ops of the event being compared.
        noctalia_rs_luau::dispatch(&ctx, &sources)?;
        let ops = noctalia_rs_luau::dispatch(&ctx, &sources)?;
        Ok(filter_ops(ops))
    }

    /// The first dispatch: startup render plus the measured event. The VM
    /// cache is keyed by entry name, so use a fresh name to guarantee the
    /// script's load render is replayed.
    fn first_ops(&self, side: Side) -> Result<Vec<Op>> {
        let entry = format!(
            "{}::validate-{}",
            self.fixture.plan.entry(side),
            self.event
        );
        let sources = [(entry.as_str(), self.fixture.source(side))];
        let ctx = self.context(side);
        Ok(filter_ops(noctalia_rs_luau::dispatch_entry(
            &ctx, &sources, &entry,
        )?))
    }
}

#[derive(Debug, Serialize)]
pub struct GeneratedSource {
    pub workload: &'static str,
    pub compiled_bytes: usize,
    pub handwritten_bytes: usize,
}

pub struct Generated {
    pub results: Vec<Summary>,
    pub pairs: Vec<Pair>,
    pub sources: Vec<GeneratedSource>,
}

pub fn run(options: &Options, scratch: &Path) -> Result<Generated> {
    let mut generated = Generated {
        results: Vec::new(),
        pairs: Vec::new(),
        sources: Vec::new(),
    };
    for fixture in fixtures()? {
        let dir = scratch.join(format!("generated-{}", fixture.workload()));
        fs::create_dir_all(&dir)?;
        validate(&fixture, &dir)?;

        let event = fixture.events[0];
        let run = Run {
            fixture: &fixture,
            event,
            dir: &dir,
        };
        let compiled_entry = fixture.plan.entry(Side::Compiled);
        let handwritten_entry = fixture.plan.entry(Side::Handwritten);
        let compiled_ctx = run.context(Side::Compiled);
        let handwritten_ctx = run.context(Side::Handwritten);

        let handwritten =
            measure(options.phase(handwritten_entry, false), || {
                let ops = noctalia_rs_luau::dispatch(
                    &handwritten_ctx,
                    &[(handwritten_entry, fixture.source(Side::Handwritten))],
                )?;
                std::hint::black_box(ops);
                Ok(())
            })?;
        let compiled = measure(options.phase(compiled_entry, false), || {
            let ops = noctalia_rs_luau::dispatch(
                &compiled_ctx,
                &[(compiled_entry, fixture.source(Side::Compiled))],
            )?;
            std::hint::black_box(ops);
            Ok(())
        })?;

        generated.sources.push(GeneratedSource {
            workload: fixture.workload(),
            compiled_bytes: fixture.source.len(),
            handwritten_bytes: fixture.handwritten.len(),
        });
        generated.results.push(handwritten);
        generated.results.push(compiled);
        generated.pairs.push(Pair::new(
            fixture.workload(),
            compiled,
            handwritten,
        ));
    }
    Ok(generated)
}

fn fixtures() -> Result<Vec<Fixture>> {
    Ok(vec![
        hello_bar_fixture()?,
        ui_panel_fixture()?,
        event_fixture()?,
    ])
}

fn hello_bar_fixture() -> Result<Fixture> {
    let plan =
        Plan::new("generated_hello_bar", EntryKind::Widget, "hello.state");
    let source = compile_fixture(&plan, HELLO_BAR_SOURCE)?;
    Ok(Fixture {
        plan,
        source,
        handwritten: HELLO_BAR_HANDWRITTEN,
        native: hello_bar,
        payload: json!({}),
        events: &["render", "click"],
    })
}

fn ui_panel_fixture() -> Result<Fixture> {
    let plan = Plan::new("generated_ui_panel", EntryKind::Panel, "panel.state");
    let source = compile_fixture(&plan, UI_PANEL_SOURCE)?;
    Ok(Fixture {
        plan,
        source,
        handwritten: UI_PANEL_HANDWRITTEN,
        native: ui_panel,
        payload: json!({}),
        events: &["render"],
    })
}

fn event_fixture() -> Result<Fixture> {
    let plan =
        Plan::new("generated_event", EntryKind::Service, "service.state");
    let source = compile_fixture(&plan, EVENT_SOURCE)?;
    Ok(Fixture {
        plan,
        source,
        handwritten: EVENT_HANDWRITTEN,
        native: service_event,
        payload: json!({ "id": "save" }),
        events: &["activate"],
    })
}

fn compile_fixture(plan: &Plan, handler: &str) -> Result<String> {
    let compiled = compile(
        handler,
        &CompileOptions {
            handler: "handler",
            entry: plan.workload,
            state_key: plan.state_key,
            kind: plan.kind,
            strings: &["message", "prefix", "name"],
            settings: &["message", "prefix", "name"],
            module: "main",
            modules: &[],
            native: noctalia_rs_compiler::NativeMode::Deny,
            strict: None,
        },
    )
    .map_err(|error| format!("{}: {error}", plan.workload))?;
    Ok(compiled.source)
}

/// Entry, cache and surface names are `&'static str`, so each fixture is named
/// once at startup.
fn surface(workload: &str, side: Side) -> &'static str {
    Box::leak(format!("{workload}_{}", side.name()).into_boxed_str())
}

fn validate(fixture: &Fixture, dir: &Path) -> Result<()> {
    for &event in fixture.events {
        validate_event(fixture, event, dir)?;
    }
    Ok(())
}

fn filter_ops(ops: Vec<Op>) -> Vec<Op> {
    ops.into_iter()
        .filter(|op| !matches!(op, Op::HostCall { .. }))
        .collect()
}

fn validate_event(fixture: &Fixture, event: &str, dir: &Path) -> Result<()> {
    let run = Run {
        fixture,
        event,
        dir,
    };
    let compiled = canonical(&run.first_ops(Side::Compiled)?)?;
    let handwritten = canonical(&run.first_ops(Side::Handwritten)?)?;
    if compiled != handwritten {
        return Err(format!(
            "{} {event}: compiled startup differs from hand-written startup\n\
             compiled: {compiled}\nhand-written: {handwritten}",
            fixture.workload(),
        )
        .into());
    }

    let mut native = Vec::new();
    if fixture.plan.kind.renders_on_load() {
        let load_event = match fixture.plan.kind {
            EntryKind::Service | EntryKind::Shortcut => "load",
            _ => fixture.plan.kind.update_event(),
        };
        let load = context(
            (fixture.workload(), load_event),
            dir,
            json!({ "source": "load" }),
        );
        native.extend((fixture.native)(&load)?);
    }
    native.extend((fixture.native)(&context(
        (fixture.workload(), event),
        dir,
        fixture.payload.clone(),
    ))?);
    let expected = canonical(&native)?;
    if compiled != expected {
        return Err(format!(
            "{} {event}: Luau startup and event ops differ from the native handler\n\
             compiled: {compiled}\nnative: {expected}",
            fixture.workload(),
        )
        .into());
    }

    let expected = canonical(&(fixture.native)(&context(
        (fixture.workload(), event),
        dir,
        fixture.payload.clone(),
    ))?)?;
    for side in [Side::Compiled, Side::Handwritten] {
        let actual = canonical(&run.ops(side)?)?;
        if actual != expected {
            return Err(format!(
                "{} {event}: {} event output differs from the native handler\n\
                 {}: {actual}\nnative: {expected}",
                fixture.workload(),
                side.name(),
                side.name()
            )
            .into());
        }
    }
    Ok(())
}

fn hello_bar(ctx: &Context) -> HandlerResult<Vec<Op>> {
    let message = ctx.config_str("message").unwrap_or_default().into_owned();
    if ctx.event == "render" {
        Ok(vec![Op::BarText { text: message }])
    } else if ctx.event == "click" {
        let log = format!("click {message}");
        Ok(vec![Op::BarText { text: message }, Op::log(log)])
    } else {
        Ok(Vec::new())
    }
}

fn ui_panel(ctx: &Context) -> HandlerResult<Vec<Op>> {
    let label = ctx.state_value::<String>("label").unwrap_or_default();
    let tree = ui::column(
        json!({ "id": "root", "gap": 4 }),
        vec![
            ui::row(
                json!({ "id": "header" }),
                vec![
                    ui::label(json!({ "id": "title", "text": "Status" })),
                    ui::label(json!({ "id": "value", "text": label })),
                ],
            ),
            ui::button(json!({ "id": "save", "label": "Save" })),
            ui::input(json!({ "id": "filter", "placeholder": "Filter" })),
            ui::progress(json!({ "id": "progress", "value": 0.75 })),
        ],
    );
    Ok(vec![Op::PanelRender { tree }])
}

fn service_event(ctx: &Context) -> HandlerResult<Vec<Op>> {
    if ctx.event != "activate" {
        return Ok(Vec::new());
    }
    let id = ctx.payload_str("id").unwrap_or_default();
    let prefix = ctx.config_str("prefix").unwrap_or_default().into_owned();
    let status = format!("{prefix}:{id}");
    Ok(vec![
        Op::state(
            "service.status",
            json!({ "id": id, "status": status.clone() }),
        ),
        Op::log(status.clone()),
        Op::BarText { text: status },
    ])
}
