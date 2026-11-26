mod generated;
mod metadata;
mod plugins;
mod process;
mod stats;

use chrono::Utc;
use noctalia_rs::{Context, Op, ui};
use serde::Serialize;
use serde_json::{Value, json};

use metadata::{Metadata, SamplePlan};
use stats::{Phase, Summary};

use std::env;
use std::error::Error;
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::Instant;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const SCRIPT_VM: &str = "function update() end";
const SCRIPT_RUNTIME: &str = r#"
function update(text)
  local rows = json.decode(text)
  local total = 0
  for _, row in ipairs(rows) do
    total = total + row.value
  end
  noctalia.log(tostring(total))
end
"#;
const SCRIPT_JSON: &str = r#"
function update()
  local text = json.encode({
    id = 42, name = "bench", ok = true,
    rows = { 1, 2, 3, 4, 5, 6, 7, 8 }
  })
  local value = json.decode(text)
  assert(value.id == 42, "id")
  assert(value.name == "bench", "name")
  assert(value.ok == true, "ok")
  assert(value.rows[8] == 8, "rows")
end
"#;
const SCRIPT_SNAPSHOT: &str = r##"
function update()
  assert(noctalia.getConfig("name") == "bench", "config.name")
  assert(noctalia.getConfig("count") == 3, "config.count")
  assert(state.get("label") == "ready", "state.label")
  assert(noctalia.getSetting("shell.font") == "sans", "setting")
  assert(noctalia.getColor("primary") == "#fff", "color")
  assert(noctalia.systemStats().cpu == 12, "stats")
  assert(noctalia.nowMs() == 1700000000, "now")
end
"##;
const SCRIPT_STATE_OPS: &str = r#"
function update()
  assert(state.get("label") == "ready", "label")
  assert(state.get("timers")[3] == 3, "timers")
  assert(state.get("missing") == nil, "missing")
  state.set("bench.a", { value = 1 })
  state.set("bench.b", { value = 2 })
  state.set("bench.c", { value = 3 })
  state.set("bench.d", { value = 4 })
  state.watch("bench.a", { __host_callback = "bench.callback" })
end
"#;
const SCRIPT_UI_TREE: &str = r#"
function update()
  local tree = ui.column({ id = "root", gap = 4 }, {
    ui.row({ id = "header" }, {
      ui.label({ id = "title", text = "Status" }),
      ui.label({ id = "value", text = "ready" })
    }),
    ui.scroll({ id = "list", height = 200 }, {
      ui.button({ id = "save", label = "Save" }),
      ui.input({ id = "filter", placeholder = "Filter" }),
      ui.progress({ id = "progress", value = 0.75 })
    })
  })
  panel.render(tree)
end
"#;
const SCRIPT_CALLBACK: &str = r#"
function update(base, count)
  local total = 0
  for i = 1, count do
    total = total + base * i
  end
  state.set("callback.total", total)
  noctalia.log(tostring(total))
end
"#;
const SCRIPT_SERVICE_UPDATE: &str = r#"
function update(prefix, threshold, rows)
  local total = 0
  local active = 0
  for _, row in ipairs(rows) do
    total = total + row.value
    if row.value >= threshold then
      active = active + 1
    end
  end
  local status = prefix .. ":" .. noctalia.getConfig("name") .. ":" ..
    tostring(total) .. ":" .. tostring(active)
  state.set("service.status", status)
  noctalia.log(status)
  barWidget.setText(status)
end
"#;
const FIXTURE_MANIFEST: &str = r#"
id = "bench/fixture"
name = "Bench Fixture"
plugin_api = 57
author = "Bench"
license = "MIT"
description = "Generated benchmark fixture"
tags = ["bench"]
icon = "sparkles"
dependencies = []

[[setting]]
key = "message"
type = "string"
label_key = "settings.message.label"
default = "Hello"

[[service]]
id = "service"
entry = "service.luau"
"#;
const SOURCES_VM: &[(&str, &str)] = &[("entry", SCRIPT_VM)];
const SOURCES_RUNTIME: &[(&str, &str)] = &[("entry", SCRIPT_RUNTIME)];
const SOURCES_JSON: &[(&str, &str)] = &[("entry", SCRIPT_JSON)];
const SOURCES_SNAPSHOT: &[(&str, &str)] = &[("entry", SCRIPT_SNAPSHOT)];
const SOURCES_STATE_OPS: &[(&str, &str)] = &[("entry", SCRIPT_STATE_OPS)];
const SOURCES_UI_TREE: &[(&str, &str)] = &[("entry", SCRIPT_UI_TREE)];
const SOURCES_CALLBACK: &[(&str, &str)] = &[("entry", SCRIPT_CALLBACK)];
const SOURCES_SERVICE_UPDATE: &[(&str, &str)] =
    &[("entry", SCRIPT_SERVICE_UPDATE)];

#[derive(Debug)]
struct Options {
    samples: usize,
    warmup: usize,
    build_samples: usize,
    build_warmup: usize,
    helper: Option<PathBuf>,
    report: Option<PathBuf>,
}
impl Options {
    fn plan(&self) -> SamplePlan {
        SamplePlan {
            samples: self.samples,
            warmup: self.warmup,
            build_samples: self.build_samples,
            build_warmup: self.build_warmup,
        }
    }

    fn phase(&self, surface: &'static str, build: bool) -> Phase {
        if build {
            Phase {
                surface,
                samples: self.build_samples,
                warmup: self.build_warmup,
            }
        } else {
            Phase {
                surface,
                samples: self.samples,
                warmup: self.warmup,
            }
        }
    }
}

#[derive(Debug, Serialize)]
struct Pair {
    workload: &'static str,
    rust: Summary,
    luau: Summary,
    speedup_median: f64,
    speedup_p95: f64,
    speedup_p99: f64,
}

impl Pair {
    fn new(workload: &'static str, rust: Summary, luau: Summary) -> Self {
        Self {
            workload,
            speedup_median: ratio(rust.median_us, luau.median_us),
            speedup_p95: ratio(rust.p95_us, luau.p95_us),
            speedup_p99: ratio(rust.p99_us, luau.p99_us),
            rust,
            luau,
        }
    }
}

fn ratio(rust_us: f64, luau_us: f64) -> f64 {
    if rust_us > 0.0 {
        luau_us / rust_us
    } else {
        0.0
    }
}

#[derive(Debug, Serialize)]
struct Report {
    metadata: Metadata,
    results: Vec<Summary>,
    pairs: Vec<Pair>,
    generated: Vec<generated::GeneratedSource>,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("noctalia-bench: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let options = parse_options()?;
    let workspace =
        fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))?;
    let helper = match &options.helper {
        Some(path) => fs::canonicalize(path).unwrap_or_else(|_| path.clone()),
        None => {
            build_helper(&workspace)?;
            helper_path(&workspace)
        }
    };
    if !helper.is_file() {
        return Err(
            format!("helper binary missing: {}", helper.display()).into()
        );
    }

    let scratch = Scratch::new(&workspace)?;
    let metadata = Metadata::collect(&workspace, &helper, options.plan());
    let mut results = Vec::new();

    results.push(measure(
        options.phase("fresh_process_dispatch", false),
        || process::fresh(&helper, &scratch.path().join("fresh")),
    )?);

    let mut spool = process::Spool::start(&helper, scratch.path())?;
    results.push(measure(options.phase("spool_dispatch", false), || {
        spool.request()
    })?);
    drop(spool);

    let mut vm_counter = 0usize;
    results.push(measure(options.phase("luau_vm_creation", false), || {
        let dir = scratch.path().join(format!("vm-cold/{vm_counter}"));
        vm_counter += 1;
        let ctx = context(("entry", "update"), &dir, payload("{}"));
        let ops = noctalia_rs_luau::dispatch(&ctx, SOURCES_VM)?;
        std::hint::black_box(ops);
        Ok(())
    })?);

    let vm_dir = scratch.path().join("vm-warm");
    results.push(measure(
        options.phase("luau_vm_cache_reuse", false),
        || {
            let ctx = context(("entry", "update"), &vm_dir, payload("{}"));
            let ops = noctalia_rs_luau::dispatch(&ctx, SOURCES_VM)?;
            std::hint::black_box(ops);
            Ok(())
        },
    )?);

    let runtime_dir = scratch.path().join("runtime");
    let rows = serde_json::to_string(&json!([
        { "value": 1 }, { "value": 2 }, { "value": 3 }, { "value": 4 }
    ]))?;
    results.push(measure(
        options.phase("luau_runtime_dispatch", false),
        || {
            let ctx =
                context(("entry", "update"), &runtime_dir, payload(&rows));
            let ops = noctalia_rs_luau::dispatch_entry(
                &ctx,
                SOURCES_RUNTIME,
                "entry",
            )?;
            std::hint::black_box(ops);
            Ok(())
        },
    )?);

    let mut pairs = Vec::new();

    let json_ctx = context(
        ("entry", "update"),
        &scratch.path().join("pair-json"),
        payload("{}"),
    );
    let json_luau =
        measure(options.phase("json_roundtrip_luau", false), || {
            let ops = noctalia_rs_luau::dispatch(&json_ctx, SOURCES_JSON)?;
            std::hint::black_box(ops);
            Ok(())
        })?;
    results.push(json_luau);
    let json_rust =
        measure(options.phase("json_roundtrip_rust", false), || {
            json_roundtrip_rust()
        })?;
    results.push(json_rust);
    pairs.push(Pair::new("json_roundtrip", json_rust, json_luau));

    let bridge_source = bridge_snapshot_source();
    let bridge_sources = [("entry", bridge_source.as_str())];
    let bridge_dir = scratch.path().join("bridge-snapshot");
    results.push(measure(
        options.phase("bridge_snapshot_reads", false),
        || {
            let ctx = context(("entry", "update"), &bridge_dir, payload("{}"));
            let ops = noctalia_rs_luau::dispatch(&ctx, &bridge_sources)?;
            std::hint::black_box(ops);
            Ok(())
        },
    )?);

    let event_source = bridge_event_source();
    let event_sources = [("entry", event_source.as_str())];
    let event_dir = scratch.path().join("bridge-event");
    results.push(measure(options.phase("bridge_event", false), || {
        let ctx = event_context(&event_dir);
        let ops = noctalia_rs_luau::dispatch(&ctx, &event_sources)?;
        std::hint::black_box(ops);
        Ok(())
    })?);

    let sample = sample_value();
    let encoded = serde_json::to_string(&sample)?;
    results.push(measure(options.phase("json_encode_rust", false), || {
        let output = serde_json::to_string(&sample)?;
        std::hint::black_box(output);
        Ok(())
    })?);
    results.push(measure(options.phase("json_decode_rust", false), || {
        let output = serde_json::from_str::<Value>(&encoded)?;
        std::hint::black_box(output);
        Ok(())
    })?);

    let snapshot_rust_ctx = context(
        ("entry", "update"),
        &scratch.path().join("snapshot-rust"),
        payload("{}"),
    );
    let snapshot_rust =
        measure(options.phase("snapshot_reads_rust", false), || {
            snapshot_reads_rust(&snapshot_rust_ctx)
        })?;
    results.push(snapshot_rust);
    let snapshot_luau_ctx = context(
        ("entry", "update"),
        &scratch.path().join("snapshot-luau"),
        payload("{}"),
    );
    let snapshot_luau =
        measure(options.phase("snapshot_reads_luau", false), || {
            let ops = noctalia_rs_luau::dispatch(
                &snapshot_luau_ctx,
                SOURCES_SNAPSHOT,
            )?;
            std::hint::black_box(ops);
            Ok(())
        })?;
    results.push(snapshot_luau);
    pairs.push(Pair::new("snapshot_reads", snapshot_rust, snapshot_luau));

    let state_ctx = context(
        ("entry", "update"),
        &scratch.path().join("pair-state"),
        payload("{}"),
    );
    let state_luau = measure(options.phase("state_ops_luau", false), || {
        let ops = noctalia_rs_luau::dispatch(&state_ctx, SOURCES_STATE_OPS)?;
        std::hint::black_box(ops);
        Ok(())
    })?;
    results.push(state_luau);
    let state_rust = measure(options.phase("state_ops_rust", false), || {
        let ops = state_ops_rust(&state_ctx)?;
        std::hint::black_box(ops);
        Ok(())
    })?;
    results.push(state_rust);
    pairs.push(Pair::new("state_ops", state_rust, state_luau));

    let ui_ctx = context(
        ("entry", "update"),
        &scratch.path().join("pair-ui"),
        payload("{}"),
    );
    let ui_luau = measure(options.phase("ui_tree_luau", false), || {
        let ops = noctalia_rs_luau::dispatch(&ui_ctx, SOURCES_UI_TREE)?;
        std::hint::black_box(ops);
        Ok(())
    })?;
    results.push(ui_luau);
    let ui_rust = measure(options.phase("ui_tree_rust", false), || {
        let ops = ui_tree_rust();
        std::hint::black_box(ops);
        Ok(())
    })?;
    results.push(ui_rust);
    pairs.push(Pair::new("ui_tree", ui_rust, ui_luau));

    let callback_ctx = context(
        ("entry", "update"),
        &scratch.path().join("pair-callback"),
        json!({ "args": [3, 4] }),
    );
    let callback_luau =
        measure(options.phase("callback_dispatch_luau", false), || {
            let ops =
                noctalia_rs_luau::dispatch(&callback_ctx, SOURCES_CALLBACK)?;
            std::hint::black_box(ops);
            Ok(())
        })?;
    results.push(callback_luau);
    let callback_rust =
        measure(options.phase("callback_dispatch_rust", false), || {
            let ops = callback_dispatch_rust(&callback_ctx)?;
            std::hint::black_box(ops);
            Ok(())
        })?;
    results.push(callback_rust);
    pairs.push(Pair::new("callback_dispatch", callback_rust, callback_luau));

    let service_ctx = context(
        ("entry", "update"),
        &scratch.path().join("pair-service"),
        json!({
            "args": [
                "bench",
                5,
                [
                    { "name": "cpu", "value": 12 },
                    { "name": "mem", "value": 64 },
                    { "name": "disk", "value": 3 },
                    { "name": "net", "value": 8 },
                    { "name": "gpu", "value": 21 },
                    { "name": "temp", "value": 7 },
                    { "name": "load", "value": 15 },
                    { "name": "io", "value": 2 }
                ]
            ]
        }),
    );
    let service_luau =
        measure(options.phase("service_update_luau", false), || {
            let ops = noctalia_rs_luau::dispatch(
                &service_ctx,
                SOURCES_SERVICE_UPDATE,
            )?;
            std::hint::black_box(ops);
            Ok(())
        })?;
    results.push(service_luau);
    let service_rust =
        measure(options.phase("service_update_rust", false), || {
            let ops = service_update_rust(&service_ctx)?;
            std::hint::black_box(ops);
            Ok(())
        })?;
    results.push(service_rust);
    pairs.push(Pair::new("service_update", service_rust, service_luau));

    plugins::run(&options, scratch.path(), &mut results, &mut pairs)?;
    let generated = generated::run(&options, scratch.path())?;
    results.extend(generated.results);
    pairs.extend(generated.pairs);

    let mut fixture = Fixture::new(&workspace, scratch.path())?;
    results.push(measure(options.phase("codegen_generate", true), || {
        noctalia_rs_build::generate_to(&fixture.manifest, &fixture.out_dir)?;
        Ok(())
    })?);
    results.push(measure(
        options.phase("build_incremental_release", true),
        || fixture.rebuild(),
    )?);

    let report = Report {
        metadata,
        results,
        pairs,
        generated: generated.sources,
    };
    print_report(&report);
    if let Some(path) = &options.report {
        fs::write(path, serde_json::to_string_pretty(&report)?)?;
    }
    Ok(())
}

fn measure<F>(phase: Phase, mut run: F) -> Result<Summary>
where
    F: FnMut() -> Result<()>,
{
    for _ in 0..phase.warmup {
        run()?;
    }
    let mut times = Vec::with_capacity(phase.samples);
    for _ in 0..phase.samples {
        let start = Instant::now();
        run()?;
        times.push(start.elapsed());
    }
    Ok(Summary::new(phase, times))
}

fn context(
    names: (&str, &str),
    data_dir: &Path,
    payload: Value,
) -> noctalia_rs::Context {
    noctalia_rs::Context::new(
        noctalia_rs::Parts {
            entry: names.0.to_owned(),
            event: names.1.to_owned(),
            payload,
            config: sample_config(),
            state: sample_state(),
            host: sample_host(),
            data_dir: data_dir.to_path_buf(),
        },
        json!({ "greet": { "label": "Hello {name}" } }),
    )
}
fn json_roundtrip_rust() -> Result<()> {
    let value = json!({
        "id": 42, "name": "bench", "ok": true,
        "rows": [1, 2, 3, 4, 5, 6, 7, 8]
    });
    let text = serde_json::to_string(&value)?;
    let decoded: Value = serde_json::from_str(&text)?;
    assert_eq!(decoded["id"], 42);
    assert_eq!(decoded["name"], "bench");
    assert_eq!(decoded["ok"], true);
    assert_eq!(decoded["rows"][7], 8);
    std::hint::black_box(decoded);
    Ok(())
}

fn snapshot_reads_rust(ctx: &Context) -> Result<()> {
    assert_eq!(ctx.config_str("name").as_deref(), Some("bench"));
    assert_eq!(ctx.config_str("count").as_deref(), Some("3"));
    assert_eq!(ctx.state_value::<String>("label")?, "ready");
    assert_eq!(
        ctx.setting("shell.font").and_then(Value::as_str),
        Some("sans")
    );
    assert_eq!(ctx.color("primary").and_then(Value::as_str), Some("#fff"));
    assert_eq!(ctx.host("stats.cpu").and_then(Value::as_i64), Some(12));
    assert_eq!(ctx.now_ms, 1_700_000_000);
    Ok(())
}

fn state_ops_rust(ctx: &Context) -> Result<Vec<Op>> {
    assert_eq!(ctx.state_value::<String>("label")?, "ready");
    let timers = ctx.state_value::<Vec<i64>>("timers")?;
    assert_eq!(timers.get(2), Some(&3));
    assert!(ctx.state.get("missing").is_none());

    let ops = vec![
        Op::state("bench.a", json!({ "value": 1 })),
        Op::state("bench.b", json!({ "value": 2 })),
        Op::state("bench.c", json!({ "value": 3 })),
        Op::state("bench.d", json!({ "value": 4 })),
        Op::state_watch("bench.a", "bench.callback"),
    ];
    std::hint::black_box(&ops);
    Ok(ops)
}

fn ui_tree_rust() -> Vec<Op> {
    let tree = ui::column(
        json!({ "id": "root", "gap": 4 }),
        vec![
            ui::row(
                json!({ "id": "header" }),
                vec![
                    ui::label(json!({ "id": "title", "text": "Status" })),
                    ui::label(json!({ "id": "value", "text": "ready" })),
                ],
            ),
            ui::scroll(
                json!({ "id": "list", "height": 200 }),
                vec![
                    ui::button(json!({ "id": "save", "label": "Save" })),
                    ui::input(
                        json!({ "id": "filter", "placeholder": "Filter" }),
                    ),
                    ui::progress(json!({ "id": "progress", "value": 0.75 })),
                ],
            ),
        ],
    );
    vec![Op::PanelRender { tree }]
}

fn callback_dispatch_rust(ctx: &Context) -> Result<Vec<Op>> {
    let base = ctx.payload_arg(0).and_then(Value::as_i64).unwrap_or(0);
    let count = ctx.payload_arg(1).and_then(Value::as_i64).unwrap_or(0);
    let mut total = 0;
    for index in 1..=count {
        total += base * index;
    }
    Ok(vec![
        Op::state("callback.total", json!(total)),
        Op::log(total.to_string()),
    ])
}

fn service_update_rust(ctx: &Context) -> Result<Vec<Op>> {
    let prefix = ctx.payload_arg(0).and_then(Value::as_str).unwrap_or("");
    let threshold = ctx.payload_arg(1).and_then(Value::as_i64).unwrap_or(0);
    let rows = ctx.payload_arg(2).ok_or("service update has no rows")?;
    let mut total = 0;
    let mut active = 0;
    for row in rows.as_array().ok_or("service rows are not an array")? {
        let value = row.get("value").and_then(Value::as_i64).unwrap_or(0);
        total += value;
        if value >= threshold {
            active += 1;
        }
    }
    let name = ctx.config_str("name").unwrap_or_default();
    let status = format!("{prefix}:{name}:{total}:{active}");
    Ok(vec![
        Op::state("service.status", json!(status)),
        Op::log(status.clone()),
        Op::BarText { text: status },
    ])
}

fn bridge_snapshot_source() -> String {
    let bridge = noctalia_rs::bridge::SOURCE
        .replace("{{HELPER}}", "bench-helper")
        .replace("{{PREFIX}}", "bench");
    format!(
        "local bridge = (function()\n{bridge}\nend)()\n\
         bridge.setup(\"service\", {{\"name\"}})\n\
         function update()\n  bridge.call(\"update\", {{}})\nend\n"
    )
}

fn payload(text: &str) -> Value {
    json!({ "args": [text] })
}

fn sample_config() -> Value {
    json!({
        "name": "bench",
        "count": 3,
        "message": "Hello",
        "prefix": "bench",
    })
}

fn sample_state() -> Value {
    json!({ "label": "ready", "timers": [1, 2, 3] })
}

fn sample_host() -> Value {
    json!({
        "now_ms": 1700000000_i64,
        "colors": { "primary": "#fff" },
        "setting": { "shell.font": "sans" },
        "stats": { "cpu": 12 }
    })
}

// Mirrors the setting paths and color roles `bridge.luau` reads, so the host
// snapshot a bridged event ships has the shape it has in production.
const BRIDGE_SETTINGS: [&str; 17] = [
    "shell.time_format",
    "shell.date_format",
    "shell.offline_mode",
    "shell.lang",
    "shell.language",
    "shell.corner_radius_scale",
    "shell.show_close_buttons",
    "general.language",
    "bar",
    "bar.order",
    "widget",
    "desktop_widgets",
    "desktop_widgets.widget",
    "theme.source",
    "theme.pure_black_dark",
    "theme.wallpaper_scheme",
    "system.monitor.network_poll_seconds",
];

const BRIDGE_COLORS: [&str; 30] = [
    "primary",
    "on_primary",
    "primary_container",
    "on_primary_container",
    "secondary",
    "on_secondary",
    "secondary_container",
    "on_secondary_container",
    "tertiary",
    "on_tertiary",
    "tertiary_container",
    "on_tertiary_container",
    "error",
    "on_error",
    "error_container",
    "on_error_container",
    "surface",
    "on_surface",
    "on_surface_variant",
    "surface_container_low",
    "surface_container",
    "surface_container_high",
    "surface_container_highest",
    "outline_variant",
    "shadow",
    "scrim",
    "hover",
    "on_hover",
    "surface_variant",
    "outline",
];

fn event_host() -> Value {
    let setting = BRIDGE_SETTINGS
        .iter()
        .map(|path| ((*path).to_owned(), json!("value")))
        .collect::<serde_json::Map<String, Value>>();
    let colors = BRIDGE_COLORS
        .iter()
        .map(|role| ((*role).to_owned(), json!("#aabbcc")))
        .collect::<serde_json::Map<String, Value>>();
    json!({
        "now_ms": 1700000000_i64,
        "dark": true,
        "outputs": [{ "name": "DP-1", "width": 1920, "height": 1080 }],
        "focused_output_name": "DP-1",
        "clipboard": "clip",
        "media": { "title": "Song", "artist": "Artist", "status": "playing" },
        "stats": { "cpu": 12 },
        "nightlight": Value::Null,
        "calendar": Value::Null,
        "compositor": { "name": "niri" },
        "setting": setting,
        "bar": { "vertical": false, "output_name": "DP-1" },
        "colors": colors,
    })
}

fn event_context(data_dir: &Path) -> Context {
    Context::new(
        noctalia_rs::Parts {
            entry: "entry".to_owned(),
            event: "update".to_owned(),
            payload: payload("{}"),
            config: sample_config(),
            state: sample_state(),
            host: event_host(),
            data_dir: data_dir.to_path_buf(),
        },
        json!({ "greet": { "label": "Hello {name}" } }),
    )
}

/// Builds the bridge into a host double so one sample measures what the bridge
/// assembles and encodes for an event. The host's own write is not part of it.
fn bridge_event_source() -> String {
    let bridge = noctalia_rs::bridge::SOURCE
        .replace("{{HELPER}}", "bench-helper")
        .replace("{{PREFIX}}", "bench");
    format!(
        "noctalia.mkdirAll = function() return true end\n\
         noctalia.runStream = function(_, on_line)\n\
         on_line('{{\"control\":\"ready\",\"boot\":\"bench\",\"spool\":\"/bench/spool\"}}')\n\
         return true\n\
         end\n\
         noctalia.writeFile = function(_, contents)\n\
         assert(#contents > 0, \"empty bridge request\")\n\
         return true\n\
         end\n\
         noctalia.renameFile = function() return true end\n\
         local bridge = (function()\n{bridge}\nend)()\n\
         bridge.setup(\"service\", {{\"name\"}})\n\
         function update()\n  bridge.call(\"update\", {{}})\nend\n"
    )
}

fn sample_value() -> Value {
    json!({
        "id": 42,
        "name": "bench",
        "enabled": true,
        "rows": [
            { "id": 1, "label": "one" },
            { "id": 2, "label": "two" },
            { "id": 3, "label": "three" }
        ],
        "nested": { "tags": ["a", "b"], "ratio": 0.75 }
    })
}

fn helper_path(workspace: &Path) -> PathBuf {
    let target = env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace.join("target"));
    target.join("release/noctalia-template")
}

fn build_helper(workspace: &Path) -> Result<()> {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(cargo)
        .args(["build", "--release", "--quiet", "-p", "noctalia-template"])
        .arg("--manifest-path")
        .arg(workspace.join("Cargo.toml"))
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;
    if !status.success() {
        return Err("failed to build release helper".into());
    }
    Ok(())
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(workspace: &Path) -> Result<Self> {
        let root = workspace.join(".tmp");
        fs::create_dir_all(&root)?;
        let name = format!(
            "bench-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let path = root.join(name);
        fs::create_dir_all(&path)?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Fixture {
    manifest: PathBuf,
    out_dir: PathBuf,
    cargo: PathBuf,
    root: PathBuf,
    source: PathBuf,
    target_dir: PathBuf,
    revision: u64,
}

impl Fixture {
    fn new(workspace: &Path, scratch: &Path) -> Result<Self> {
        let root = scratch.join("fixture");
        fs::create_dir_all(root.join("src"))?;
        let out_dir = scratch.join("generated");
        fs::create_dir_all(&out_dir)?;

        let deps = workspace.display().to_string();
        let manifest = format!(
            "[package]\nname = \"noctalia-bench-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n\n\
             [dependencies]\nnoctalia-rs = {{ path = \"{deps}/crates/lib\" }}\nnoctalia-rs-luau = {{ path = \"{deps}/crates/luau\" }}\nserde_json = \"1\"\n\n\
             [build-dependencies]\nnoctalia-rs-build = {{ path = \"{deps}/crates/build\" }}\n\n[workspace]\n"
        );
        fs::write(root.join("Cargo.toml"), manifest)?;
        fs::write(
            root.join("build.rs"),
            "fn main() -> Result<(), Box<dyn std::error::Error>> {\n    noctalia_rs_build::generate()\n}\n",
        )?;
        fs::write(root.join("noctalia.toml"), FIXTURE_MANIFEST)?;
        fs::write(
            root.join("service.luau"),
            "function update()\n  noctalia.log(noctalia.getConfig(\"message\"))\nend\n",
        )?;
        let source = root.join("src/main.rs");
        fs::write(
            &source,
            "include!(concat!(env!(\"OUT_DIR\"), \"/generated.rs\"));\nfn main() {}\n",
        )?;

        // An environment variable rather than a flag: CI points every commit at
        // its own directory, and commits older than this option still run.
        let target_dir = env::var_os("NOCTALIA_BENCH_FIXTURE_TARGET")
            .map_or_else(|| root.join("target"), PathBuf::from);
        Ok(Self {
            manifest: root.join("noctalia.toml"),
            out_dir,
            cargo: env::var_os("CARGO")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("cargo")),
            root,
            source,
            target_dir,
            revision: 0,
        })
    }

    fn rebuild(&mut self) -> Result<()> {
        self.revision += 1;
        let mut file = OpenOptions::new().append(true).open(&self.source)?;
        writeln!(file, "// revision {}", self.revision)?;
        drop(file);

        let mut command = Command::new(&self.cargo);
        command
            .args([
                "build",
                "--release",
                "--quiet",
                "--offline",
                "--manifest-path",
            ])
            .arg(self.root.join("Cargo.toml"))
            .env("CARGO_TARGET_DIR", &self.target_dir)
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let output = command.output()?;
        if output.status.success() {
            return Ok(());
        }
        let retry = Command::new(&self.cargo)
            .args(["build", "--release", "--quiet", "--manifest-path"])
            .arg(self.root.join("Cargo.toml"))
            .env("CARGO_TARGET_DIR", &self.target_dir)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()?;
        if retry.status.success() {
            return Ok(());
        }
        Err(format!(
            "fixture build failed: {}",
            String::from_utf8_lossy(&retry.stderr)
        )
        .into())
    }
}

fn parse_options() -> Result<Options> {
    let mut options = Options {
        samples: 200,
        warmup: 20,
        build_samples: 5,
        build_warmup: 1,
        helper: None,
        report: None,
    };
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!("{}", usage());
                std::process::exit(0);
            }
            "--samples" => options.samples = number(&mut args, "--samples")?,
            "--warmup" => options.warmup = number(&mut args, "--warmup")?,
            "--build-samples" => {
                options.build_samples = number(&mut args, "--build-samples")?
            }
            "--build-warmup" => {
                options.build_warmup = number(&mut args, "--build-warmup")?
            }
            "--helper" => {
                options.helper =
                    Some(PathBuf::from(next(&mut args, "--helper")?))
            }
            "--report" => {
                options.report =
                    Some(PathBuf::from(next(&mut args, "--report")?))
            }
            other => {
                return Err(
                    format!("unknown option {other}. {}", usage()).into()
                );
            }
        }
    }
    if options.samples == 0 || options.build_samples == 0 {
        return Err("sample counts must be positive".into());
    }
    Ok(options)
}

fn next(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    args.next()
        .ok_or_else(|| format!("missing value for {flag}").into())
}

fn number(
    args: &mut impl Iterator<Item = String>,
    flag: &str,
) -> Result<usize> {
    let value = next(args, flag)?;
    value
        .parse::<usize>()
        .map_err(|_| format!("invalid value for {flag}: {value}").into())
}

fn usage() -> String {
    let mut text = String::new();
    let _ = writeln!(text, "usage: noctalia-bench [OPTIONS]");
    let _ = writeln!(
        text,
        "  --samples N        samples per runtime surface (default 200)"
    );
    let _ = writeln!(
        text,
        "  --warmup N         warmups per runtime surface (default 20)"
    );
    let _ = writeln!(
        text,
        "  --build-samples N  samples per build surface (default 5)"
    );
    let _ = writeln!(
        text,
        "  --build-warmup N   warmups per build surface (default 1)"
    );
    let _ = writeln!(
        text,
        "  --helper PATH      use an existing release helper binary"
    );
    let _ = writeln!(text, "  --report PATH      write a JSON report");
    let _ = writeln!(
        text,
        "  NOCTALIA_BENCH_FIXTURE_TARGET=DIR\n                     reuse the fixture build directory between runs"
    );
    text
}

fn print_report(report: &Report) {
    let meta = &report.metadata;
    println!("timestamp: {}", meta.timestamp_utc);
    println!("machine: {} | {}", meta.os, meta.cpu_model);
    println!(
        "cpus: {} | max {} kHz | memory {} KiB",
        meta.logical_cpus, meta.cpu_max_freq_khz, meta.memory_total_kib
    );
    println!(
        "toolchain: {} | {} | profile {}",
        meta.rustc, meta.cargo, meta.profile
    );
    println!(
        "workspace: {} | git {} | dirty {}",
        meta.workspace, meta.git_head, meta.git_dirty
    );
    println!("helper: {}", meta.helper);
    println!(
        "samples: {} (warmup {}), build samples: {} (warmup {})",
        meta.samples, meta.warmup, meta.build_samples, meta.build_warmup
    );
    println!(
        "method: median (mean of two middle values for even n), {} p95/p99, {}",
        meta.percentile_method, meta.timer
    );
    println!();
    println!(
        "{:<28} {:>7} {:>6} {:>12} {:>12} {:>12} {:>10} {:>10}",
        "surface",
        "samples",
        "warmup",
        "median_us",
        "p95_us",
        "p99_us",
        "min_us",
        "max_us"
    );
    for item in &report.results {
        println!(
            "{:<28} {:>7} {:>6} {:>12.3} {:>12.3} {:>12.3} {:>10.3} {:>10.3}",
            item.surface,
            item.samples,
            item.warmup,
            item.median_us,
            item.p95_us,
            item.p99_us,
            item.min_us,
            item.max_us
        );
    }
    println!();
    println!(
        "Matched Rust vs Luau workloads (speedup = Luau median / Rust median)"
    );
    println!(
        "{:<20} {:>10} {:>10} {:>10} {:>10} {:>10} {:>10} {:>9} {:>7}",
        "workload",
        "rust_med",
        "rust_p95",
        "rust_p99",
        "luau_med",
        "luau_p95",
        "luau_p99",
        "speedup",
        "target"
    );
    for pair in report
        .pairs
        .iter()
        .filter(|pair| !pair.workload.starts_with("generated_"))
    {
        println!(
            "{:<20} {:>10.3} {:>10.3} {:>10.3} {:>10.3} {:>10.3} {:>10.3} {:>9.2} {:>7}",
            pair.workload,
            pair.rust.median_us,
            pair.rust.p95_us,
            pair.rust.p99_us,
            pair.luau.median_us,
            pair.luau.p95_us,
            pair.luau.p99_us,
            pair.speedup_median,
            target(pair),
        );
    }
    if !report.generated.is_empty() {
        println!();
        println!(
            "Generated Luau: compiled from the Rust handler vs hand-written"
        );
        println!(
            "{:<24} {:>10} {:>10} {:>10} {:>10} {:>9} {:>14} {:>17}",
            "workload",
            "compiled",
            "compiled95",
            "hand",
            "hand95",
            "speedup",
            "compiled_bytes",
            "handwritten_bytes"
        );
        for item in &report.generated {
            let (compiled, compiled95, hand, hand95, speedup) =
                generated_times(&report.pairs, item.workload);
            println!(
                "{:<24} {:>10.3} {:>10.3} {:>10.3} {:>10.3} {:>8.2}x {:>14} {:>17}",
                item.workload,
                compiled,
                compiled95,
                hand,
                hand95,
                speedup,
                item.compiled_bytes,
                item.handwritten_bytes
            );
        }
        println!(
            "compiled/hand are medians in microseconds; speedup = hand / compiled. Target: compiled Luau >= hand-written Luau (parity). Both sides pay the same ~2 us dispatch floor, so the 3x target applies to spool_dispatch, not to this comparison."
        );
    }
}

fn generated_times(
    pairs: &[Pair],
    workload: &str,
) -> (f64, f64, f64, f64, f64) {
    let Some(pair) = pairs.iter().find(|pair| pair.workload == workload) else {
        return (0.0, 0.0, 0.0, 0.0, 0.0);
    };
    (
        pair.rust.median_us,
        pair.rust.p95_us,
        pair.luau.median_us,
        pair.luau.p95_us,
        pair.speedup_median,
    )
}

fn target(pair: &Pair) -> &'static str {
    if pair.workload.starts_with("generated_") {
        return if pair.speedup_median >= 1.0 {
            "parity"
        } else {
            "slower"
        };
    }
    if pair.speedup_median >= 3.0 {
        "3x yes"
    } else {
        "3x no"
    }
}
