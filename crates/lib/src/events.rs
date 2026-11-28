//! Typed events for each entry kind.
//!
//! The bridge hands the handler a string event name and a JSON payload. A
//! plugin can keep reading those fields directly, or it can decode them once
//! into one of the enums here. Each enum lists the events the host delivers to
//! that entry kind, so a `match` covers the whole surface without repeating
//! payload key strings.
//!
//! The compiler recognizes `ctx.<kind>_event()` as a pure event read. In every
//! compile mode the read folds to the event label already available to the
//! generated branch, and a matched payload binds the decoded payload table.
//! Unknown event names stay visible as `Unknown(String)` instead of being
//! coerced into a known variant.

use serde_json::Value;

use crate::Context;

/// The `source` carried by update, render and load events.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourcePayload {
    pub source: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScrollPayload {
    pub axis: Option<String>,
    pub steps: Option<f64>,
    pub starts_gesture: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HoverPayload {
    pub hovered: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct KeyPayload {
    pub chord: Option<String>,
    pub pressed: Option<bool>,
    pub repeated: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct IpcPayload {
    pub event: Option<String>,
    pub payload: Option<Value>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExitPayload {
    pub signal: Option<f64>,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioSpectrumPayload {
    pub values_csv: Option<String>,
    pub state_csv: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct OpenPayload {
    pub context: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FramePayload {
    pub delta: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ActivatePayload {
    pub id: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchPayload {
    pub text: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ActionPayload {
    pub name: Option<String>,
    pub args: Vec<Value>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HostCallbackPayload {
    pub id: Option<String>,
    pub args: Vec<Value>,
}

fn text(ctx: &Context, key: &str) -> Option<String> {
    ctx.payload_str(key)
}

fn boolean(ctx: &Context, key: &str) -> Option<bool> {
    ctx.payload.get(key).and_then(Value::as_bool)
}

fn number(ctx: &Context, key: &str) -> Option<f64> {
    ctx.payload.get(key).and_then(Value::as_f64)
}

fn value(ctx: &Context, key: &str) -> Option<Value> {
    ctx.payload.get(key).cloned().filter(|item| !item.is_null())
}

fn args(ctx: &Context) -> Vec<Value> {
    ctx.payload
        .get("args")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn source(ctx: &Context) -> SourcePayload {
    SourcePayload {
        source: text(ctx, "source"),
    }
}

fn scroll(ctx: &Context) -> ScrollPayload {
    ScrollPayload {
        axis: text(ctx, "axis"),
        steps: number(ctx, "steps"),
        starts_gesture: boolean(ctx, "starts_gesture"),
    }
}

fn hover(ctx: &Context) -> HoverPayload {
    HoverPayload {
        hovered: boolean(ctx, "hovered"),
    }
}

fn key(ctx: &Context) -> KeyPayload {
    KeyPayload {
        chord: text(ctx, "chord"),
        pressed: boolean(ctx, "pressed"),
        repeated: boolean(ctx, "repeated"),
    }
}

fn ipc(ctx: &Context) -> IpcPayload {
    IpcPayload {
        event: text(ctx, "event"),
        payload: value(ctx, "payload"),
    }
}

fn exit(ctx: &Context) -> ExitPayload {
    ExitPayload {
        signal: number(ctx, "signal"),
        reason: text(ctx, "reason"),
    }
}

fn audio_spectrum(ctx: &Context) -> AudioSpectrumPayload {
    AudioSpectrumPayload {
        values_csv: text(ctx, "values_csv"),
        state_csv: text(ctx, "state_csv"),
    }
}

fn open(ctx: &Context) -> OpenPayload {
    OpenPayload {
        context: text(ctx, "context"),
    }
}

fn frame(ctx: &Context) -> FramePayload {
    FramePayload {
        delta: number(ctx, "delta"),
    }
}

fn activate(ctx: &Context) -> ActivatePayload {
    ActivatePayload {
        id: text(ctx, "id"),
    }
}

fn search(ctx: &Context) -> SearchPayload {
    SearchPayload {
        text: text(ctx, "text"),
    }
}

fn action(ctx: &Context) -> ActionPayload {
    ActionPayload {
        name: text(ctx, "name"),
        args: args(ctx),
    }
}

fn host_callback(ctx: &Context) -> HostCallbackPayload {
    HostCallbackPayload {
        id: text(ctx, "id"),
        args: args(ctx),
    }
}

macro_rules! typed_event {
    (
        $name:ident {
            payload: [$($variant:ident($payload:ty) => $label:literal => $decode:ident,)*]
            unit: [$($unit:ident => $unit_label:literal,)*]
        }
    ) => {
        #[derive(Clone, Debug, PartialEq)]
        pub enum $name {
            $($variant($payload),)*
            $($unit,)*
            Unknown(String),
        }

        impl $name {
            #[inline]
            #[must_use]
            pub fn from_context(ctx: &Context) -> Self {
                match ctx.event.as_str() {
                    $($label => Self::$variant($decode(ctx)),)*
                    $($unit_label => Self::$unit,)*
                    other => Self::Unknown(other.to_owned()),
                }
            }
        }
    };
}

typed_event!(ServiceEvent {
    payload: [
        Update(SourcePayload) => "update" => source,
        Ipc(IpcPayload) => "ipc" => ipc,
        Exit(ExitPayload) => "exit" => exit,
        AudioSpectrum(AudioSpectrumPayload) => "audio_spectrum" => audio_spectrum,
        Action(ActionPayload) => "action" => action,
        HostCallback(HostCallbackPayload) => "host_callback" => host_callback,
    ]
    unit: [
        Load => "load",
        ConfigChanged => "config_changed",
        Enable => "enable",
        Disable => "disable",
        Suspend => "suspend",
        Resume => "resume",
        Refresh => "refresh",
    ]
});

typed_event!(WidgetEvent {
    payload: [
        Render(SourcePayload) => "render" => source,
        Scroll(ScrollPayload) => "scroll" => scroll,
        Hover(HoverPayload) => "hover" => hover,
        Key(KeyPayload) => "key" => key,
        Ipc(IpcPayload) => "ipc" => ipc,
        Exit(ExitPayload) => "exit" => exit,
        AudioSpectrum(AudioSpectrumPayload) => "audio_spectrum" => audio_spectrum,
        Open(OpenPayload) => "open" => open,
        Action(ActionPayload) => "action" => action,
        HostCallback(HostCallbackPayload) => "host_callback" => host_callback,
    ]
    unit: [
        Click => "click",
        RightClick => "right_click",
        MiddleClick => "middle_click",
        DoubleClick => "double_click",
        ConfigChanged => "config_changed",
        OutputsChanged => "outputs_changed",
        Refresh => "refresh",
        Close => "close",
    ]
});

typed_event!(ShortcutEvent {
    payload: [
        Render(SourcePayload) => "render" => source,
        Key(KeyPayload) => "key" => key,
        Action(ActionPayload) => "action" => action,
        HostCallback(HostCallbackPayload) => "host_callback" => host_callback,
    ]
    unit: [
        Load => "load",
        Click => "click",
        RightClick => "right_click",
        MiddleClick => "middle_click",
        DoubleClick => "double_click",
        ConfigChanged => "config_changed",
    ]
});

typed_event!(DesktopWidgetEvent {
    payload: [
        Render(SourcePayload) => "render" => source,
        Scroll(ScrollPayload) => "scroll" => scroll,
        Hover(HoverPayload) => "hover" => hover,
        Action(ActionPayload) => "action" => action,
        HostCallback(HostCallbackPayload) => "host_callback" => host_callback,
    ]
    unit: [
        Click => "click",
        RightClick => "right_click",
        MiddleClick => "middle_click",
        DoubleClick => "double_click",
        ConfigChanged => "config_changed",
        OutputsChanged => "outputs_changed",
    ]
});

typed_event!(LauncherEvent {
    payload: [
        Query(SearchPayload) => "query" => search,
        Activate(ActivatePayload) => "activate" => activate,
        Search(SearchPayload) => "search" => search,
        SearchChange(SearchPayload) => "search_change" => search,
        Update(SourcePayload) => "update" => source,
        Ipc(IpcPayload) => "ipc" => ipc,
        Action(ActionPayload) => "action" => action,
        HostCallback(HostCallbackPayload) => "host_callback" => host_callback,
    ]
    unit: [
        ConfigChanged => "config_changed",
    ]
});

typed_event!(PanelEvent {
    payload: [
        Render(SourcePayload) => "render" => source,
        Open(OpenPayload) => "open" => open,
        Key(KeyPayload) => "key" => key,
        Action(ActionPayload) => "action" => action,
        HostCallback(HostCallbackPayload) => "host_callback" => host_callback,
    ]
    unit: [
        Click => "click",
        RightClick => "right_click",
        MiddleClick => "middle_click",
        DoubleClick => "double_click",
        Close => "close",
        CloseClicked => "close_clicked",
        RefreshClicked => "refresh_clicked",
        SettingsClicked => "settings_clicked",
        OpenSettings => "open_settings",
        ClosePanel => "close_panel",
        ConfigChanged => "config_changed",
        OutputsChanged => "outputs_changed",
        Refresh => "refresh",
    ]
});

typed_event!(LayerEvent {
    payload: [
        Render(SourcePayload) => "render" => source,
        Frame(FramePayload) => "frame" => frame,
        Key(KeyPayload) => "key" => key,
        Hover(HoverPayload) => "hover" => hover,
        Action(ActionPayload) => "action" => action,
        HostCallback(HostCallbackPayload) => "host_callback" => host_callback,
    ]
    unit: [
        Click => "click",
        RightClick => "right_click",
        MiddleClick => "middle_click",
        DoubleClick => "double_click",
        OutputsChanged => "outputs_changed",
    ]
});

impl Context {
    #[inline]
    #[must_use]
    pub fn service_event(&self) -> ServiceEvent {
        ServiceEvent::from_context(self)
    }

    #[inline]
    #[must_use]
    pub fn widget_event(&self) -> WidgetEvent {
        WidgetEvent::from_context(self)
    }

    #[inline]
    #[must_use]
    pub fn shortcut_event(&self) -> ShortcutEvent {
        ShortcutEvent::from_context(self)
    }

    #[inline]
    #[must_use]
    pub fn desktop_widget_event(&self) -> DesktopWidgetEvent {
        DesktopWidgetEvent::from_context(self)
    }

    #[inline]
    #[must_use]
    pub fn launcher_event(&self) -> LauncherEvent {
        LauncherEvent::from_context(self)
    }

    #[inline]
    #[must_use]
    pub fn panel_event(&self) -> PanelEvent {
        PanelEvent::from_context(self)
    }

    #[inline]
    #[must_use]
    pub fn layer_event(&self) -> LayerEvent {
        LayerEvent::from_context(self)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::*;
    use crate::Parts;

    fn context(event: &str, payload: Value) -> Context {
        Context::new(
            Parts {
                entry: "test".to_owned(),
                event: event.to_owned(),
                payload,
                config: json!({}),
                state: json!({}),
                host: json!({}),
                data_dir: PathBuf::from("/tmp"),
            },
            json!({}),
        )
    }

    #[test]
    fn decodes_scroll_payload() {
        let ctx = context(
            "scroll",
            json!({ "axis": "y", "steps": -3, "starts_gesture": true }),
        );
        assert_eq!(
            ctx.widget_event(),
            WidgetEvent::Scroll(ScrollPayload {
                axis: Some("y".to_owned()),
                steps: Some(-3.0),
                starts_gesture: Some(true),
            })
        );
    }

    #[test]
    fn decodes_action_args_without_null_slots() {
        let ctx = context(
            "action",
            json!({ "name": "refresh", "args": ["a", 1, true] }),
        );
        assert_eq!(
            ctx.panel_event(),
            PanelEvent::Action(ActionPayload {
                name: Some("refresh".to_owned()),
                args: vec![json!("a"), json!(1), json!(true)],
            })
        );
    }

    #[test]
    fn decodes_host_callback_payload() {
        let ctx = context(
            "host_callback",
            json!({ "id": "load", "args": [{ "ok": true }] }),
        );
        assert_eq!(
            ctx.service_event(),
            ServiceEvent::HostCallback(HostCallbackPayload {
                id: Some("load".to_owned()),
                args: vec![json!({ "ok": true })],
            })
        );
    }

    #[test]
    fn keeps_unknown_events_visible() {
        let ctx = context("mystery", json!({}));
        assert_eq!(
            ctx.layer_event(),
            LayerEvent::Unknown("mystery".to_owned())
        );
    }

    #[test]
    fn decodes_render_source() {
        let ctx = context("render", json!({ "source": "load" }));
        assert_eq!(
            ctx.widget_event(),
            WidgetEvent::Render(SourcePayload {
                source: Some("load".to_owned()),
            })
        );
    }
}
