mod emit;
mod error;
mod events;
mod program;
pub mod strict;

use std::collections::BTreeMap;
use std::fmt::Write as _;

use syn::spanned::Spanned;
use syn::{FnArg, Pat, Type};

pub use error::Error;
pub use strict::StrictOptions;

use emit::Emitter;
use program::Program;

/// The callback table mirrored by `noctalia_rs::codegen::CALLBACKS`.
///
/// The compiler emits the same host calls as the bridge, so the two tables
/// must agree. `crates/lib` asserts that in a test.
pub const CALLBACKS: &[Callback] = &[
    Callback {
        event: "click",
        function: "onClick",
        params: "",
        payload: "",
    },
    Callback {
        event: "right_click",
        function: "onRightClick",
        params: "",
        payload: "",
    },
    Callback {
        event: "middle_click",
        function: "onMiddleClick",
        params: "",
        payload: "",
    },
    Callback {
        event: "double_click",
        function: "onDoubleClick",
        params: "",
        payload: "",
    },
    Callback {
        event: "scroll",
        function: "onScroll",
        params: "axis, steps, startsGesture",
        payload: "{ axis = axis, steps = steps, starts_gesture = startsGesture }",
    },
    Callback {
        event: "hover",
        function: "onHover",
        params: "hovered",
        payload: "{ hovered = hovered }",
    },
    Callback {
        event: "key",
        function: "onKey",
        params: "chord, pressed, repeated",
        payload: "{ chord = chord, pressed = pressed, repeated = repeated }",
    },
    Callback {
        event: "ipc",
        function: "onIpc",
        params: "event, payload",
        payload: "{ event = event, payload = payload }",
    },
    Callback {
        event: "config_changed",
        function: "onConfigChanged",
        params: "",
        payload: "",
    },
    Callback {
        event: "outputs_changed",
        function: "onOutputsChanged",
        params: "",
        payload: "",
    },
    Callback {
        event: "enable",
        function: "onEnable",
        params: "",
        payload: "",
    },
    Callback {
        event: "disable",
        function: "onDisable",
        params: "",
        payload: "",
    },
    Callback {
        event: "exit",
        function: "onExit",
        params: "signal, reason",
        payload: "{ signal = signal, reason = reason }",
    },
    Callback {
        event: "audio_spectrum",
        function: "onAudioSpectrum",
        params: "valuesCsv, stateCsv",
        payload: "{ values_csv = valuesCsv, state_csv = stateCsv }",
    },
    Callback {
        event: "suspend",
        function: "onSuspend",
        params: "",
        payload: "",
    },
    Callback {
        event: "resume",
        function: "onResume",
        params: "",
        payload: "",
    },
    Callback {
        event: "refresh",
        function: "onRefresh",
        params: "",
        payload: "",
    },
    Callback {
        event: "open",
        function: "onOpen",
        params: "context",
        payload: "{ context = context }",
    },
    Callback {
        event: "close",
        function: "onClose",
        params: "",
        payload: "",
    },
    Callback {
        event: "close_clicked",
        function: "onCloseClicked",
        params: "",
        payload: "",
    },
    Callback {
        event: "refresh_clicked",
        function: "onRefreshClicked",
        params: "",
        payload: "",
    },
    Callback {
        event: "settings_clicked",
        function: "onSettingsClicked",
        params: "",
        payload: "",
    },
    Callback {
        event: "open_settings",
        function: "onOpenSettings",
        params: "",
        payload: "",
    },
    Callback {
        event: "close_panel",
        function: "onClosePanel",
        params: "",
        payload: "",
    },
    Callback {
        event: "frame",
        function: "onFrameTick",
        params: "delta",
        payload: "{ delta = delta }",
    },
    Callback {
        event: "query",
        function: "onQuery",
        params: "text",
        payload: "",
    },
    Callback {
        event: "activate",
        function: "onActivate",
        params: "id",
        payload: "{ id = id }",
    },
    Callback {
        event: "search",
        function: "onSearch",
        params: "text",
        payload: "{ text = text }",
    },
    Callback {
        event: "search_change",
        function: "onSearchChange",
        params: "text",
        payload: "{ text = text }",
    },
];

#[derive(Clone, Copy, Debug)]
pub struct Callback {
    pub event: &'static str,
    pub function: &'static str,
    pub params: &'static str,
    pub payload: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntryKind {
    Service,
    Widget,
    Shortcut,
    DesktopWidget,
    Launcher,
    Panel,
    Layer,
}

impl EntryKind {
    #[must_use]
    pub fn update_event(self) -> &'static str {
        match self {
            Self::Service | Self::Launcher => "update",
            Self::Widget
            | Self::Shortcut
            | Self::DesktopWidget
            | Self::Panel
            | Self::Layer => "render",
        }
    }

    #[must_use]
    pub fn watches_state(self) -> bool {
        !matches!(self, Self::Service)
    }

    #[must_use]
    pub fn renders_on_load(self) -> bool {
        !matches!(self, Self::Launcher)
    }
}

/// One extra source file in the crate, at its Rust module path.
///
/// `path` is `main`, `widget`, `common::util` or `common` for a `mod.rs`.
#[derive(Clone, Copy, Debug)]
pub struct Module<'a> {
    pub path: &'a str,
    pub source: &'a str,
}

pub struct Options<'a> {
    pub handler: &'a str,
    pub entry: &'a str,
    pub state_key: &'a str,
    pub kind: EntryKind,
    /// Config keys declared as strings in the manifest. `config_str` on these
    /// keys needs no runtime coercion.
    pub strings: &'a [&'a str],
    /// Every config key declared in the manifest. A native function that takes
    /// `&Context` sees the same config map the bridge sends.
    pub settings: &'a [&'a str],
    /// Module path of `source`, such as `main` or `widget`.
    ///
    /// An empty `modules` list keeps the single-file behavior: helpers defined
    /// in `source` are inlined and nothing else is visible. `module` names the
    /// file that receives helper errors.
    pub module: &'a str,
    /// The rest of the crate, so calls into sibling modules can be inlined.
    pub modules: &'a [Module<'a>],
    /// Whether a function the compiler cannot translate may cross into the
    /// helper process as a native call. `strict` denies it.
    pub native: NativeMode,
    /// Strict-mode data from the build. `Some` selects strict emission:
    /// no bridge, no helper, direct member calls, and the synthesized
    /// context tables. `None` keeps auto and off output unchanged.
    pub strict: Option<StrictOptions<'a>>,
}

/// Whether functions may cross into the helper process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeMode {
    /// Demote an untranslatable function to a native call and keep compiling.
    Allow,
    /// Fail on the first untranslatable function, as `strict` does.
    Deny,
}

/// How a native function receives one parameter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeParamKind {
    /// Deserialized from the request arguments.
    Value,
    /// The handler context, rebuilt in the helper from the request.
    Context,
}

/// How the dispatch passes a deserialized local to the function.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativePass {
    /// Pass the local by value.
    Value,
    /// Pass `&local`.
    Ref,
    /// Pass `&local[..]`.
    Slice,
    /// Pass `local.as_ref()`.
    AsRef,
    /// Pass `local.as_deref()`.
    AsDeref,
}

/// One argument of a native function.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeParam {
    /// Parameter name in the Rust signature.
    pub name: String,
    /// Whether the parameter travels as JSON or as the handler context.
    pub kind: NativeParamKind,
    /// Owned type for the local that receives the deserialized value, when the
    /// declared parameter is a reference that needs one (`&str` -> `String`).
    pub owned: Option<String>,
    /// How to pass the local to the function.
    pub pass: NativePass,
}

/// One function that runs in the helper process instead of in Luau.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeFn {
    /// Dispatch key, such as `main::fetch_prs` or `main::Git::load`.
    pub name: String,
    /// Module that defines the function, such as `main` or `git::api`.
    pub module: String,
    /// Function identifier.
    pub function: String,
    /// Parameters, in declaration order.
    pub params: Vec<NativeParam>,
    /// Implementing type for an inherent method.
    pub class: Option<String>,
    /// Whether the declared return type is `Result<...>`.
    pub result: bool,
    /// In-helper cache lifetime in milliseconds, `0` for no cache.
    pub cache_ms: u64,
    /// Why the function crossed the boundary.
    pub reason: String,
}

#[derive(Clone, Debug)]
pub struct Compiled {
    pub source: String,
    pub events: Vec<String>,
    /// Functions that cross into the helper process.
    pub native: Vec<NativeFn>,
}

/// Compiles one handler in `source` into a complete Luau entry script.
///
/// The handler must take a single `&Context` argument and return
/// `Result<Vec<Op>>`. Constructs outside the supported subset are reported as
/// errors with their source position instead of being translated silently.
pub fn compile(source: &str, options: &Options<'_>) -> Result<Compiled, Error> {
    let program = Program::new(options.module, source, options.modules)?;
    let handler = program.handler(options.module, options.handler)?;
    let mut known: BTreeMap<String, NativeFn> = BTreeMap::new();
    loop {
        let (compiled, found) = compile_once(
            &program,
            handler.function,
            &handler.module,
            options,
            &known,
        )?;
        let mut grew = false;
        for (name, item) in found {
            if let std::collections::btree_map::Entry::Vacant(slot) =
                known.entry(name)
            {
                slot.insert(item);
                grew = true;
            }
        }
        if !grew {
            return Ok(compiled);
        }
    }
}

type CompiledOnce = (Compiled, BTreeMap<String, NativeFn>);

fn compile_once(
    program: &Program,
    function: &syn::ItemFn,
    module: &str,
    options: &Options<'_>,
    known: &BTreeMap<String, NativeFn>,
) -> Result<CompiledOnce, Error> {
    let module = module.to_owned();
    let context = context_name(function)?;
    let mut discovery = Emitter::new(
        context,
        None,
        options.strings,
        &[],
        program,
        &module,
        options.native,
        known,
        options.strict,
        true,
        true,
    );
    discovery.constants();
    discovery.block(&function.block)?;
    let discovery_body = discovery.finish();
    let events = discovery_body.events.clone();
    let negatives = discovery_body.negative_events.clone();
    let has_action = discovery_body.events.contains("action");
    let has_host_callback = discovery_body.events.contains("host_callback");
    let positive = events.difference(&negatives).cloned().collect::<Vec<_>>();

    let update_event = options.kind.update_event().to_owned();
    let mut labels = vec![update_event.clone()];
    for event in &positive {
        if event != &update_event {
            labels.push(event.clone());
        }
    }
    if matches!(options.kind, EntryKind::Service | EntryKind::Shortcut)
        && !labels.iter().any(|label| label == "load")
    {
        labels.push("load".to_owned());
    }

    struct Slot {
        name: String,
        body: emit::Body,
    }

    let mut bodies: Vec<Slot> = Vec::new();
    let mut names = std::collections::BTreeMap::new();
    for label in &labels {
        let mut emitter = Emitter::new(
            context,
            Some(label.clone()),
            options.strings,
            emit::payload_params(label),
            program,
            &module,
            options.native,
            known,
            options.strict,
            has_action,
            has_host_callback,
        );
        emitter.constants();
        emitter.block(&function.block)?;
        let body = emitter.finish();
        let existing = bodies
            .iter()
            .find(|slot| slot.body.source == body.source)
            .map(|slot| slot.name.clone());
        let name = if let Some(name) = existing {
            name
        } else {
            let name = format!("body_{}", sanitize(label));
            bodies.push(Slot {
                name: name.clone(),
                body,
            });
            name
        };
        names.insert(label.clone(), name);
    }

    let fallback_index = if !discovery_body.is_empty {
        let index = bodies.len();
        bodies.push(Slot {
            name: "body_events".to_owned(),
            body: discovery_body,
        });
        Some(index)
    } else {
        None
    };
    let slot = |label: &str| -> Option<&Slot> {
        names
            .get(label)
            .and_then(|name| bodies.iter().find(|slot| &slot.name == name))
    };
    let update = slot(&update_event).filter(|slot| !slot.body.is_empty);
    let fallback = fallback_index.map(|index| &bodies[index]);
    let watch_label = if options.kind == EntryKind::Launcher {
        "query"
    } else {
        update_event.as_str()
    };
    let watch = options
        .kind
        .watches_state()
        .then(|| slot(watch_label).or(fallback))
        .flatten()
        .filter(|slot| !slot.body.is_empty);
    let load_label =
        if matches!(options.kind, EntryKind::Service | EntryKind::Shortcut) {
            "load"
        } else {
            update_event.as_str()
        };
    let load = options
        .kind
        .renders_on_load()
        .then(|| slot(load_label))
        .flatten()
        .filter(|slot| !slot.body.is_empty);

    let uses_state =
        bodies.iter().any(|slot| slot.body.uses_state) || watch.is_some();
    let uses_entry = bodies.iter().any(|slot| slot.body.uses_entry);
    let uses_irem = bodies.iter().any(|slot| slot.body.uses_irem);
    let uses_unwrap = bodies.iter().any(|slot| slot.body.uses_unwrap);
    let uses_host_call = bodies.iter().any(|slot| slot.body.uses_host_call);
    let uses_bool_or = bodies.iter().any(|slot| slot.body.uses_bool_or);
    let uses_replace = bodies.iter().any(|slot| slot.body.uses_replace);
    let hoists = bodies
        .iter()
        .any(|slot| slot.body.uses_payload && !slot.body.payload_escapes);

    let mut out = String::new();
    out.push_str("--!nonstrict\n");
    out.push_str(
        "-- Generated from Rust by noctalia-rs. Edit the Rust handler instead.\n\n",
    );
    if uses_state {
        let _ = writeln!(out, "local STATE_KEY = {}", quote(options.state_key));
    }
    if uses_entry {
        let _ = writeln!(out, "local ENTRY = {}", quote(options.entry));
    }
    let mut statics = std::collections::BTreeMap::new();
    for slot in &bodies {
        for (name, init) in &slot.body.statics {
            statics.insert(name.clone(), init.clone());
        }
    }
    for name in statics.keys() {
        let _ = writeln!(out, "local {name}");
    }
    if hoists {
        out.push_str("local PAYLOAD_EMPTY = {}\n");
        out.push_str("local PAYLOAD_UPDATE = { source = \"update\" }\n");
        if options.kind.watches_state() {
            out.push_str("local PAYLOAD_STATE = { source = \"state\" }\n");
        }
        if options.kind.renders_on_load() {
            out.push_str("local PAYLOAD_LOAD = { source = \"load\" }\n");
        }
    }
    if uses_irem {
        out.push_str(concat!(
            "local function irem(a, b)\n",
            "  local r = a % b\n",
            "  if r ~= 0 and (a < 0) ~= (b < 0) then r = r + b end\n",
            "  return r\n",
            "end\n",
        ));
    }
    if uses_unwrap {
        out.push_str(concat!(
            "local function unwrap(value)\n",
            "  if value == nil then error(\"called unwrap on nil\") end\n",
            "  return value\n",
            "end\n",
        ));
    }
    if uses_bool_or {
        out.push_str(concat!(
            "local function bool_or(value, default)\n",
            "  if type(value) == \"boolean\" then return value end\n",
            "  return default\n",
            "end\n",
        ));
    }
    if uses_replace {
        out.push_str(concat!(
            "local function replace_all(text, from, to)\n",
            "  if from == \"\" then return text end\n",
            "  local parts, start = {}, 1\n",
            "  while true do\n",
            "    local found = string.find(text, from, start, true)\n",
            "    if not found then break end\n",
            "    parts[#parts + 1] = string.sub(text, start, found - 1)\n",
            "    parts[#parts + 1] = to\n",
            "    start = found + #from\n",
            "  end\n",
            "  parts[#parts + 1] = string.sub(text, start)\n",
            "  return table.concat(parts)\n",
            "end\n",
        ));
    }
    let uses_query = bodies.iter().any(|slot| slot.body.uses_query);
    if uses_host_call || uses_query {
        out.push_str(concat!(
            "local ROOTS = { noctalia = noctalia, barWidget = barWidget, desktopWidget = desktopWidget, shortcut = shortcut, panel = panel, launcher = launcher, layer = layer, ui = ui }\n",
            "local function resolve_root(root, path)\n",
            "  local target = ROOTS[root]\n",
            "  for _, part in ipairs(path) do\n",
            "    if type(target) ~= \"table\" then return nil end\n",
            "    target = target[part]\n",
            "  end\n",
            "  return target\n",
            "end\n",
        ));
    }
    if uses_host_call {
        out.push_str(concat!(
            "local function host_call(root, path, args)\n",
            "  local target = resolve_root(root, path)\n",
            "  if type(target) ~= \"function\" then return false end\n",
            "  return pcall(target, table.unpack(args))\n",
            "end\n",
        ));
    }
    if bodies.iter().any(|slot| slot.body.uses_join) {
        out.push_str(concat!(
            "local function join(value, separator)\n",
            "  if type(value) == \"table\" then return table.concat(value, separator) end\n",
            "  local base = tostring(value)\n",
            "  if string.sub(base, -1) == \"/\" then return base .. separator end\n",
            "  return base .. \"/\" .. separator\n",
            "end\n",
        ));
    }
    if uses_query {
        out.push_str(concat!(
            "local function host_query(root, path, args)\n",
            "  local target = resolve_root(root, path)\n",
            "  if type(target) ~= \"function\" then return nil end\n",
            "  return target(table.unpack(args))\n",
            "end\n",
        ));
    }
    if options.strict.is_some() {
        if bodies.iter().any(|slot| slot.body.uses_unpack) {
            out.push_str("local __unpack = table.unpack\n");
        }
        if bodies.iter().any(|slot| slot.body.uses_pcall) {
            out.push_str("local __pcall = pcall\n");
        }
    }
    if let Some(strict) = &options.strict {
        let uses_config = bodies.iter().any(|slot| slot.body.uses_config_table);
        let uses_translations =
            bodies.iter().any(|slot| slot.body.uses_translations_table);
        if uses_config {
            out.push_str(strict.config);
        }
        if uses_translations {
            out.push_str(strict.translations);
        }
        if (uses_config || uses_translations) && !out.ends_with('\n') {
            out.push('\n');
        }
    }
    let mut natives = known.clone();
    for slot in &bodies {
        for item in &slot.body.native {
            natives
                .entry(item.name.clone())
                .or_insert_with(|| item.clone());
        }
    }
    if !natives.is_empty() {
        out.push_str(&emit::native_runtime(
            options.entry,
            options.state_key,
            options.settings,
        ));
        for item in natives.values() {
            out.push_str(&emit::stub_source(item));
        }
    }
    for (name, init) in &statics {
        let _ = writeln!(out, "{name} = {init}");
    }
    if !out.ends_with("\n\n") {
        out.push('\n');
    }

    for slot in &bodies {
        if slot.body.is_empty {
            continue;
        }
        let _ = writeln!(
            out,
            "local function {}({})",
            slot.name,
            body_params(&slot.body)
        );
        out.push_str(&slot.body.source);
        out.push_str("end\n\n");
    }

    if let Some(slot) = update {
        let args = call_args(
            &slot.body,
            &[],
            &[],
            Some("update"),
            None,
            false,
            &update_event,
        );
        let _ = writeln!(
            out,
            "function update() {} end",
            invoke(
                &slot.body,
                &slot.name,
                &args,
                Some(descriptor(&update_event, Some("update"), None, false))
            )
        );
    } else {
        out.push_str("function update() end\n");
    }

    if let Some(slot) = slot("host_callback").filter(|slot| !slot.body.is_empty)
    {
        if options.strict.is_some() {
            let payload = "{ id = id, args = { ... } }";
            let args = call_args(
                &slot.body,
                &[],
                &[],
                None,
                Some(payload),
                false,
                "host_callback",
            );
            let _ = writeln!(
                out,
                "function {}(id, ...) {} end",
                strict::CALLBACK_DISPATCH,
                invoke(
                    &slot.body,
                    &slot.name,
                    &args,
                    Some(descriptor(
                        "host_callback",
                        None,
                        Some(payload),
                        false
                    ))
                )
            );
        } else {
            let args = call_args(
                &slot.body,
                &[],
                &[],
                None,
                Some("payload"),
                false,
                "host_callback",
            );
            let _ = writeln!(
                out,
                "function onHostCallback(payload) {} end",
                invoke(
                    &slot.body,
                    &slot.name,
                    &args,
                    Some(descriptor(
                        "host_callback",
                        None,
                        Some("payload"),
                        false
                    ))
                )
            );
        }
    }

    if options.kind == EntryKind::Launcher
        && let Some(slot) = watch
    {
        let mapping = emit::payload_params("query");
        let payload = call_args(
            &slot.body,
            mapping,
            &["text"],
            None,
            Some("{ text = text }"),
            false,
            "query",
        );
        let state_payload = call_args(
            &slot.body,
            mapping,
            &["last_query"],
            Some("state"),
            Some("{ text = last_query, source = \"state\" }"),
            false,
            "query",
        );
        let _ = writeln!(
            out,
            "local last_query = nil\nfunction onQuery(text) last_query = text; {} end",
            invoke(
                &slot.body,
                &slot.name,
                &payload,
                Some(descriptor("query", None, Some("{ text = text }"), false))
            )
        );
        let _ = writeln!(
            out,
            "state.watch(STATE_KEY, function() if last_query then {} end end)",
            invoke(
                &slot.body,
                &slot.name,
                &state_payload,
                Some(descriptor(
                    "query",
                    Some("state"),
                    Some("{ text = last_query, source = \"state\" }"),
                    false
                ))
            )
        );
    }
    for callback in CALLBACKS {
        if callback.function == "onQuery" && options.kind == EntryKind::Launcher
        {
            continue;
        }
        let Some(slot) = slot(callback.event)
            .or(fallback)
            .filter(|slot| !slot.body.is_empty)
        else {
            continue;
        };
        let dynamic =
            (!callback.payload.is_empty()).then_some(callback.payload);
        let mapping = emit::payload_params(callback.event);
        let values = mapping
            .iter()
            .map(|(_, _, value)| *value)
            .collect::<Vec<_>>();
        let args = call_args(
            &slot.body,
            mapping,
            &values,
            None,
            dynamic,
            callback.payload.is_empty(),
            callback.event,
        );
        let _ = writeln!(
            out,
            "function {}({}) {} end",
            callback.function,
            callback.params,
            invoke(
                &slot.body,
                &slot.name,
                &args,
                Some(descriptor(
                    callback.event,
                    None,
                    dynamic,
                    callback.payload.is_empty()
                ))
            )
        );
    }
    if options.strict.is_some() {
        let actions = bodies
            .iter()
            .flat_map(|slot| slot.body.actions.iter().cloned())
            .collect::<std::collections::BTreeSet<String>>();
        let dynamic_action =
            bodies.iter().any(|slot| slot.body.uses_action_dispatch);
        let notify_actions = bodies
            .iter()
            .flat_map(|slot| slot.body.notify_actions.iter().cloned())
            .collect::<std::collections::BTreeSet<String>>();
        if !(actions.is_empty() && notify_actions.is_empty() && !dynamic_action)
        {
            let Some(action) =
                slot("action").filter(|slot| !slot.body.is_empty)
            else {
                return Err(Error::plain(
                    "strict emit needs an action branch for ui::call",
                ));
            };
            for name in &actions {
                let payload =
                    format!("{{ name = {}, args = {{ ... }} }}", quote(name));
                let args = call_args(
                    &action.body,
                    &[],
                    &[],
                    None,
                    Some(&payload),
                    false,
                    "action",
                );
                let _ = writeln!(
                    out,
                    "function {}(...) {} end",
                    strict::action_global(name),
                    invoke(&action.body, &action.name, &args, None)
                );
            }
            if dynamic_action {
                let payload = "{ name = name, args = { ... } }";
                let args = call_args(
                    &action.body,
                    &[],
                    &[],
                    None,
                    Some(payload),
                    false,
                    "action",
                );
                let _ = writeln!(
                    out,
                    "function {}(name, ...) {} end",
                    strict::ACTION_DISPATCH,
                    invoke(&action.body, &action.name, &args, None)
                );
            }
            for name in &notify_actions {
                let payload = format!(
                    "{{ name = {}, args = {{ id, context }} }}",
                    quote(name)
                );
                let args = call_args(
                    &action.body,
                    &[],
                    &[],
                    None,
                    Some(&payload),
                    false,
                    "action",
                );
                let _ = writeln!(
                    out,
                    "function {name}(id, context) {} end",
                    invoke(&action.body, &action.name, &args, None)
                );
            }
        }
    }
    out.push('\n');
    if options.kind != EntryKind::Launcher
        && let Some(slot) = watch
    {
        let args = call_args(
            &slot.body,
            &[],
            &[],
            Some("state"),
            None,
            false,
            &update_event,
        );
        let _ = writeln!(
            out,
            "state.watch(STATE_KEY, function() {} end)",
            invoke(
                &slot.body,
                &slot.name,
                &args,
                Some(descriptor(&update_event, Some("state"), None, false))
            )
        );
    }
    if options.kind == EntryKind::Panel {
        out.push_str("panel.setWantsSecondTicks(true)\n");
    }
    if let Some(slot) = load {
        let args = call_args(
            &slot.body,
            &[],
            &[],
            Some("load"),
            None,
            false,
            load_label,
        );
        let _ = writeln!(
            out,
            "{}",
            invoke(
                &slot.body,
                &slot.name,
                &args,
                Some(descriptor(load_label, Some("load"), None, false))
            )
        );
    }

    Ok((
        Compiled {
            source: out,
            events: positive,
            native: natives.values().cloned().collect(),
        },
        natives,
    ))
}

/// Calls a body directly, or through the native coroutine driver when the body
/// reaches a native function.
fn invoke(
    body: &emit::Body,
    name: &str,
    args: &str,
    descriptor: Option<String>,
) -> String {
    if !body.uses_native {
        return format!("{name}({args})");
    }
    let descriptor = match (body.uses_native_context, descriptor) {
        (true, Some(descriptor)) => descriptor,
        _ => "nil".to_owned(),
    };
    if args.is_empty() {
        format!("__native.run({name}, {descriptor})")
    } else {
        format!("__native.run({name}, {descriptor}, {args})")
    }
}

/// The event and payload a native `&Context` parameter must see.
fn descriptor(
    event: &str,
    source: Option<&str>,
    dynamic: Option<&str>,
    empty: bool,
) -> String {
    let payload = if let Some(table) = dynamic {
        table.to_owned()
    } else if empty {
        "{}".to_owned()
    } else {
        format!("{{ source = {} }}", quote(source.unwrap_or("")))
    };
    format!("{{ event = {}, payload = {payload} }}", quote(event))
}

pub(crate) fn sanitize(value: &str) -> String {
    let mut out = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || character == '_' {
            out.push(character);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push_str("body");
    }
    out
}

fn body_params(body: &emit::Body) -> String {
    let mut params = body.params.clone();
    if body.uses_payload {
        params.push("payload");
    }
    if body.uses_source {
        params.push("source");
    }
    if body.uses_event {
        params.push("event");
    }
    params.join(", ")
}

fn call_args(
    body: &emit::Body,
    mapping: &[emit::PayloadParam],
    values: &[&str],
    source: Option<&str>,
    dynamic: Option<&str>,
    empty: bool,
    event: &str,
) -> String {
    let mut args = Vec::new();
    for ((_, body_param, _), value) in mapping.iter().zip(values) {
        if body.params.contains(body_param) {
            args.push((*value).to_owned());
        }
    }
    if body.uses_payload {
        args.push(wrapper_payload(body, source.unwrap_or(""), dynamic, empty));
    }
    if body.uses_source {
        args.push(source.map_or_else(|| "\"\"".to_owned(), quote));
    }
    if body.uses_event {
        args.push(quote(event));
    }
    args.join(", ")
}

fn wrapper_payload(
    body: &emit::Body,
    source: &str,
    dynamic: Option<&str>,
    empty: bool,
) -> String {
    if !body.uses_payload {
        return "nil".to_owned();
    }
    if let Some(table) = dynamic {
        return table.to_owned();
    }
    if empty {
        return if body.payload_escapes {
            "{}".to_owned()
        } else {
            "PAYLOAD_EMPTY".to_owned()
        };
    }
    if body.payload_escapes {
        return format!("{{ source = {} }}", quote(source));
    }
    match source {
        "update" => "PAYLOAD_UPDATE".to_owned(),
        "state" => "PAYLOAD_STATE".to_owned(),
        "load" => "PAYLOAD_LOAD".to_owned(),
        _ => "PAYLOAD_EMPTY".to_owned(),
    }
}

fn context_name(function: &syn::ItemFn) -> Result<&syn::Ident, Error> {
    let mut typed = function.sig.inputs.iter().filter_map(|arg| {
        let FnArg::Typed(arg) = arg else {
            return None;
        };
        Some(arg)
    });
    let Some(arg) = typed.next() else {
        return Err(Error::at(
            function.sig.ident.span(),
            "handler must take one `&Context` argument",
        ));
    };
    if typed.next().is_some() {
        return Err(Error::at(
            function.sig.ident.span(),
            "handler must take exactly one `&Context` argument",
        ));
    }
    if !is_context(&arg.ty) {
        return Err(Error::at(
            arg.ty.span(),
            "handler argument must be `&Context`",
        ));
    }
    let Pat::Ident(pattern) = &*arg.pat else {
        return Err(Error::at(
            arg.pat.span(),
            "handler argument must be a plain identifier",
        ));
    };
    Ok(&pattern.ident)
}

pub(crate) fn is_context(ty: &Type) -> bool {
    let Type::Reference(reference) = ty else {
        return false;
    };
    matches!(&*reference.elem, Type::Path(path) if path.path.segments.last().is_some_and(|segment| segment.ident == "Context"))
}

pub(crate) fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len().saturating_add(2));
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other if other.is_control() => {
                let _ = write!(out, "\\{}", other as u32);
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

pub(crate) fn is_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

pub(crate) fn is_luau_keyword(value: &str) -> bool {
    matches!(
        value,
        "and"
            | "break"
            | "do"
            | "else"
            | "elseif"
            | "end"
            | "false"
            | "for"
            | "function"
            | "goto"
            | "if"
            | "in"
            | "local"
            | "nil"
            | "not"
            | "or"
            | "repeat"
            | "return"
            | "then"
            | "true"
            | "until"
            | "while"
    )
}

#[cfg(test)]
mod tests;
