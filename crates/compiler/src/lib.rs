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

    let mut emitter = Emitter::new(context);
    emitter.constants(&file.items);
    emitter.block(&function.block)?;
    let body = emitter.finish();
    let callbacks_use_payload = body.uses_payload;
    let hoisted_payloads = callbacks_use_payload && !body.payload_escapes;

    let mut out = String::new();
    out.push_str("--!nonstrict\n");
    out.push_str(
        "-- Generated from Rust by noctalia-rs. Edit the Rust handler instead.\n\n",
    );
    let needs_state_key = body.uses_state || options.kind.watches_state();
    if needs_state_key {
        let _ = writeln!(out, "local STATE_KEY = {}", quote(options.state_key));
    }
    if body.uses_entry {
        let _ = writeln!(out, "local ENTRY = {}", quote(options.entry));
    }
    if hoisted_payloads {
        out.push_str("local PAYLOAD_EMPTY = {}\n");
        out.push_str("local PAYLOAD_UPDATE = { source = \"update\" }\n");
        if options.kind.watches_state() {
            out.push_str("local PAYLOAD_STATE = { source = \"state\" }\n");
        }
        if options.kind.renders_on_load() {
            out.push_str("local PAYLOAD_LOAD = { source = \"load\" }\n");
        }
    }
    if body.uses_irem {
        out.push_str(concat!(
            "local function irem(a, b)\n",
            "  local r = a % b\n",
            "  if r ~= 0 and (a < 0) ~= (b < 0) then r = r + b end\n",
            "  return r\n",
            "end\n",
        ));
    }
    if body.uses_unwrap {
        out.push_str(concat!(
            "local function unwrap(value)\n",
            "  if value == nil then error(\"called unwrap on nil\") end\n",
            "  return value\n",
            "end\n",
        ));
    }
    if body.uses_bool_or {
        out.push_str(concat!(
            "local function bool_or(value, default)\n",
            "  if value == nil then return default end\n",
            "  return value\n",
            "end\n",
        ));
    }
    if body.uses_host_call {
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
    out.push('\n');
    out.push_str("local function body(event, payload, source)\n");
    out.push_str(&body.source);
    out.push_str("end\n\n");

    let update_payload =
        payload("update", callbacks_use_payload, hoisted_payloads);
    let _ = writeln!(
        out,
        "function update() body({}, {update_payload}, \"update\") end",
        quote(options.kind.update_event())
    );
    if options.kind == EntryKind::Launcher {
        let query_payload = if callbacks_use_payload {
            "{ text = text }"
        } else {
            "nil"
        };
        let _ = writeln!(
            out,
            "local last_query = nil\nfunction onQuery(text) last_query = text; body(\"query\", {query_payload}, \"\") end"
        );
    }
    for callback in CALLBACKS {
        if callback.function == "onQuery" && options.kind == EntryKind::Launcher
        {
            continue;
        }
        if !body.events.iter().any(|event| event == callback.event) {
            continue;
        }
        let argument = if callback.payload.is_empty() {
            if hoisted_payloads {
                "PAYLOAD_EMPTY"
            } else if callbacks_use_payload {
                "{}"
            } else {
                "nil"
            }
        } else if callbacks_use_payload {
            callback.payload
        } else {
            "nil"
        };
        let _ = writeln!(
            out,
            "function {}({}) body({}, {argument}, \"\") end",
            callback.function,
            callback.params,
            quote(callback.event)
        );
    }
    out.push('\n');

    if options.kind == EntryKind::Launcher {
        let state_payload = if callbacks_use_payload {
            "{ text = last_query }"
        } else {
            "nil"
        };
        let _ = writeln!(
            out,
            "noctalia.state.watch(STATE_KEY, function() if last_query then body(\"query\", {state_payload}, \"state\") end end)"
        );
    } else if options.kind.watches_state() {
        let state_payload =
            payload("state", callbacks_use_payload, hoisted_payloads);
        let _ = writeln!(
            out,
            "noctalia.state.watch(STATE_KEY, function() body({}, {state_payload}, \"state\") end)",
            quote(options.kind.update_event())
        );
    }
    if options.kind == EntryKind::Panel {
        out.push_str("panel.setWantsSecondTicks(true)\n");
    }
    if options.kind.renders_on_load() {
        let event = match options.kind {
            EntryKind::Service | EntryKind::Shortcut => "load",
            EntryKind::Widget
            | EntryKind::DesktopWidget
            | EntryKind::Panel
            | EntryKind::Layer => options.kind.update_event(),
            EntryKind::Launcher => options.kind.update_event(),
        };
        let load_payload =
            payload("load", callbacks_use_payload, hoisted_payloads);
        let _ =
            writeln!(out, "body({}, {load_payload}, \"load\")", quote(event));
    }

    Ok(Compiled {
        source: out,
        events: body.events.into_iter().collect(),
    })
}

fn payload(source: &str, used: bool, hoisted: bool) -> String {
    if !used {
        return "nil".to_owned();
    }
    if hoisted {
        return format!(
            "PAYLOAD_{}",
            match source {
                "update" => "UPDATE",
                "state" => "STATE",
                "load" => "LOAD",
                other => other,
            }
        );
    }
    format!("{{ source = {} }}", quote(source))
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
