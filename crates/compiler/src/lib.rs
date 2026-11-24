mod emit;
mod error;

use std::fmt::Write as _;

use syn::spanned::Spanned;
use syn::{FnArg, Item, Pat, Type};

pub use error::Error;

use emit::Emitter;

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

pub struct Options<'a> {
    pub handler: &'a str,
    pub entry: &'a str,
    pub state_key: &'a str,
    pub kind: EntryKind,
    /// Config keys declared as strings in the manifest. `config_str` on these
    /// keys needs no runtime coercion.
    pub strings: &'a [&'a str],
}

#[derive(Clone, Debug)]
pub struct Compiled {
    pub source: String,
    pub events: Vec<String>,
}

/// Compiles one handler in `source` into a complete Luau entry script.
///
/// The handler must take a single `&Context` argument and return
/// `Result<Vec<Op>>`. Constructs outside the supported subset are reported as
/// errors with their source position instead of being translated silently.
pub fn compile(source: &str, options: &Options<'_>) -> Result<Compiled, Error> {
    let file = syn::parse_file(source)?;
    let function = find_handler(&file, options.handler)?;
    let context = context_name(function)?;

    let mut discovery = Emitter::new(context, None, options.strings, &[]);
    discovery.constants(&file.items);
    discovery.block(&function.block)?;
    let events = discovery.finish().events;

    let update_event = options.kind.update_event().to_owned();
    let mut labels = vec![update_event.clone()];
    for event in &events {
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
        );
        emitter.constants(&file.items);
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

    let slot = |label: &str| -> Option<&Slot> {
        names
            .get(label)
            .and_then(|name| bodies.iter().find(|slot| &slot.name == name))
    };
    let update = slot(&update_event).filter(|slot| !slot.body.is_empty);
    let watch_label = if options.kind == EntryKind::Launcher {
        "query"
    } else {
        update_event.as_str()
    };
    let watch = options
        .kind
        .watches_state()
        .then(|| slot(watch_label))
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
    if uses_host_call {
        out.push_str(concat!(
            "local ROOTS = { noctalia = noctalia, barWidget = barWidget, desktopWidget = desktopWidget, shortcut = shortcut, panel = panel, launcher = launcher, layer = layer, ui = ui }\n",
            "local function host_call(root, path, args)\n",
            "  local target = ROOTS[root]\n",
            "  for _, part in ipairs(path) do\n",
            "    if type(target) ~= \"table\" then return false end\n",
            "    target = target[part]\n",
            "  end\n",
            "  if type(target) ~= \"function\" then return false end\n",
            "  return pcall(target, table.unpack(args))\n",
            "end\n",
        ));
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
        let args = call_args(&slot.body, &[], &[], Some("update"), None, false);
        let _ = writeln!(out, "function update() {}({args}) end", slot.name);
    } else {
        out.push_str("function update() end\n");
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
        );
        let state_payload = call_args(
            &slot.body,
            mapping,
            &["last_query"],
            Some("state"),
            Some("{ text = last_query, source = \"state\" }"),
            false,
        );
        let _ = writeln!(
            out,
            "local last_query = nil\nfunction onQuery(text) last_query = text; {}({payload}) end",
            slot.name
        );
        let _ = writeln!(
            out,
            "state.watch(STATE_KEY, function() if last_query then {}({state_payload}) end end)",
            slot.name
        );
    }
    for callback in CALLBACKS {
        if callback.function == "onQuery" && options.kind == EntryKind::Launcher
        {
            continue;
        }
        let Some(slot) =
            slot(callback.event).filter(|slot| !slot.body.is_empty)
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
        );
        let _ = writeln!(
            out,
            "function {}({}) {}({args}) end",
            callback.function, callback.params, slot.name
        );
    }
    out.push('\n');
    if options.kind != EntryKind::Launcher
        && let Some(slot) = watch
    {
        let args = call_args(&slot.body, &[], &[], Some("state"), None, false);
        let _ = writeln!(
            out,
            "state.watch(STATE_KEY, function() {}({args}) end)",
            slot.name
        );
    }
    if options.kind == EntryKind::Panel {
        out.push_str("panel.setWantsSecondTicks(true)\n");
    }
    if let Some(slot) = load {
        let args = call_args(&slot.body, &[], &[], Some("load"), None, false);
        let _ = writeln!(out, "{}({args})", slot.name);
    }

    Ok(Compiled {
        source: out,
        events: events.into_iter().collect(),
    })
}

fn sanitize(value: &str) -> String {
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
    params.join(", ")
}

fn call_args(
    body: &emit::Body,
    mapping: &[emit::PayloadParam],
    values: &[&str],
    source: Option<&str>,
    dynamic: Option<&str>,
    empty: bool,
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

fn find_handler<'a>(
    file: &'a syn::File,
    name: &str,
) -> Result<&'a syn::ItemFn, Error> {
    let mut found = Vec::new();
    search_items(&file.items, name, &mut found);
    match found.as_slice() {
        [] => Err(Error::plain(format!(
            "no function named `{name}` in the source file"
        ))),
        [function] => Ok(*function),
        _ => Err(Error::plain(format!(
            "handler `{name}` is defined more than once in the source file"
        ))),
    }
}

fn search_items<'a>(
    items: &'a [Item],
    name: &str,
    found: &mut Vec<&'a syn::ItemFn>,
) {
    for item in items {
        match item {
            Item::Fn(function) if function.sig.ident == name => {
                found.push(function);
            }
            Item::Mod(module) => {
                if let Some((_, items)) = &module.content {
                    search_items(items, name, found);
                }
            }
            _ => {}
        }
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

fn is_context(ty: &Type) -> bool {
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

#[cfg(test)]
mod tests;
