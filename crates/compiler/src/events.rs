//! The typed event surface mirrored from `noctalia_rs::events`.
//!
//! The compiler parses handler source without type resolution, so it cannot
//! ask the library what a variant means. This table is the wire mapping used
//! when a typed event read is the scrutinee of a match. It stays a plain string
//! table so the generated Luau has no enum tag, integer width, or nullability
//! question to answer.

/// `Context` methods that read a typed event.
pub const TYPED_EVENT_METHODS: &[&str] = &[
    "service_event",
    "widget_event",
    "shortcut_event",
    "desktop_widget_event",
    "launcher_event",
    "panel_event",
    "layer_event",
];

pub type EventKind = (&'static str, &'static [&'static str]);

/// Labels each `Context` event method can return. `Unknown` matches a label
/// only when it is outside this set.
pub const TYPED_EVENT_KINDS: &[EventKind] = &[
    (
        "service_event",
        &[
            "update",
            "load",
            "ipc",
            "exit",
            "audio_spectrum",
            "action",
            "host_callback",
            "config_changed",
            "enable",
            "disable",
            "suspend",
            "resume",
            "refresh",
        ],
    ),
    (
        "widget_event",
        &[
            "render",
            "scroll",
            "hover",
            "key",
            "ipc",
            "exit",
            "audio_spectrum",
            "open",
            "action",
            "host_callback",
            "click",
            "right_click",
            "middle_click",
            "double_click",
            "config_changed",
            "outputs_changed",
            "refresh",
            "close",
        ],
    ),
    (
        "shortcut_event",
        &[
            "render",
            "key",
            "action",
            "host_callback",
            "load",
            "click",
            "right_click",
            "middle_click",
            "double_click",
            "config_changed",
        ],
    ),
    (
        "desktop_widget_event",
        &[
            "render",
            "scroll",
            "hover",
            "action",
            "host_callback",
            "click",
            "right_click",
            "middle_click",
            "double_click",
            "config_changed",
            "outputs_changed",
        ],
    ),
    (
        "launcher_event",
        &[
            "query",
            "activate",
            "search",
            "search_change",
            "update",
            "ipc",
            "action",
            "host_callback",
            "config_changed",
        ],
    ),
    (
        "panel_event",
        &[
            "render",
            "open",
            "key",
            "action",
            "host_callback",
            "click",
            "right_click",
            "middle_click",
            "double_click",
            "close",
            "close_clicked",
            "refresh_clicked",
            "settings_clicked",
            "open_settings",
            "close_panel",
            "config_changed",
            "outputs_changed",
            "refresh",
        ],
    ),
    (
        "layer_event",
        &[
            "render",
            "frame",
            "key",
            "hover",
            "action",
            "host_callback",
            "click",
            "right_click",
            "middle_click",
            "double_click",
            "outputs_changed",
        ],
    ),
];

#[must_use]
pub fn known_labels(method: &str) -> &'static [&'static str] {
    TYPED_EVENT_KINDS
        .iter()
        .find(|(name, _)| *name == method)
        .map_or(&[], |(_, labels)| labels)
}

/// Payload structs the typed variants carry, with the field types the emitted
/// Luau must use. `type_hint` turns these into the same `Option` types the Rust
/// decoders return, so `unwrap_or_default` keeps its Rust meaning.
pub type PayloadField = (&'static str, &'static str);
pub type PayloadStruct = (&'static str, &'static [PayloadField]);

pub const PAYLOAD_STRUCTS: &[PayloadStruct] = &[
    ("SourcePayload", &[("source", "Option<String>")]),
    (
        "ScrollPayload",
        &[
            ("axis", "Option<String>"),
            ("steps", "Option<f64>"),
            ("starts_gesture", "Option<bool>"),
        ],
    ),
    ("HoverPayload", &[("hovered", "Option<bool>")]),
    (
        "KeyPayload",
        &[
            ("chord", "Option<String>"),
            ("pressed", "Option<bool>"),
            ("repeated", "Option<bool>"),
        ],
    ),
    (
        "IpcPayload",
        &[
            ("event", "Option<String>"),
            ("payload", "Option<serde_json::Value>"),
        ],
    ),
    (
        "ExitPayload",
        &[("signal", "Option<f64>"), ("reason", "Option<String>")],
    ),
    (
        "AudioSpectrumPayload",
        &[
            ("values_csv", "Option<String>"),
            ("state_csv", "Option<String>"),
        ],
    ),
    ("OpenPayload", &[("context", "Option<String>")]),
    ("FramePayload", &[("delta", "Option<f64>")]),
    ("ActivatePayload", &[("id", "Option<String>")]),
    ("SearchPayload", &[("text", "Option<String>")]),
    (
        "ActionPayload",
        &[
            ("name", "Option<String>"),
            ("args", "Vec<serde_json::Value>"),
        ],
    ),
    (
        "HostCallbackPayload",
        &[("id", "Option<String>"), ("args", "Vec<serde_json::Value>")],
    ),
];

/// Maps a wire label to the payload struct its typed variant carries.
#[must_use]
pub fn payload_struct(label: &str) -> Option<&'static str> {
    match label {
        "render" | "update" => Some("SourcePayload"),
        "scroll" => Some("ScrollPayload"),
        "hover" => Some("HoverPayload"),
        "key" => Some("KeyPayload"),
        "ipc" => Some("IpcPayload"),
        "exit" => Some("ExitPayload"),
        "audio_spectrum" => Some("AudioSpectrumPayload"),
        "open" => Some("OpenPayload"),
        "frame" => Some("FramePayload"),
        "activate" => Some("ActivatePayload"),
        "query" | "search" | "search_change" => Some("SearchPayload"),
        "action" => Some("ActionPayload"),
        "host_callback" => Some("HostCallbackPayload"),
        _ => None,
    }
}
/// Maps a typed event variant to its wire label.
#[must_use]
pub fn variant_label(name: &str) -> Option<&'static str> {
    match name {
        "Render" => Some("render"),
        "Update" => Some("update"),
        "Query" => Some("query"),
        "Load" => Some("load"),
        "Click" => Some("click"),
        "RightClick" => Some("right_click"),
        "MiddleClick" => Some("middle_click"),
        "DoubleClick" => Some("double_click"),
        "Scroll" => Some("scroll"),
        "Hover" => Some("hover"),
        "Key" => Some("key"),
        "Ipc" => Some("ipc"),
        "ConfigChanged" => Some("config_changed"),
        "OutputsChanged" => Some("outputs_changed"),
        "Enable" => Some("enable"),
        "Disable" => Some("disable"),
        "Exit" => Some("exit"),
        "AudioSpectrum" => Some("audio_spectrum"),
        "Suspend" => Some("suspend"),
        "Resume" => Some("resume"),
        "Refresh" => Some("refresh"),
        "Open" => Some("open"),
        "Close" => Some("close"),
        "CloseClicked" => Some("close_clicked"),
        "RefreshClicked" => Some("refresh_clicked"),
        "SettingsClicked" => Some("settings_clicked"),
        "OpenSettings" => Some("open_settings"),
        "ClosePanel" => Some("close_panel"),
        "Frame" => Some("frame"),
        "Activate" => Some("activate"),
        "Search" => Some("search"),
        "SearchChange" => Some("search_change"),
        "Action" => Some("action"),
        "HostCallback" => Some("host_callback"),
        "Unknown" => None,
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::CALLBACKS;

    use super::*;

    #[test]
    fn every_callback_label_has_a_typed_variant() {
        for callback in CALLBACKS {
            let name = match callback.event {
                "click" => "Click",
                "right_click" => "RightClick",
                "middle_click" => "MiddleClick",
                "double_click" => "DoubleClick",
                "scroll" => "Scroll",
                "hover" => "Hover",
                "key" => "Key",
                "ipc" => "Ipc",
                "config_changed" => "ConfigChanged",
                "outputs_changed" => "OutputsChanged",
                "enable" => "Enable",
                "disable" => "Disable",
                "exit" => "Exit",
                "audio_spectrum" => "AudioSpectrum",
                "suspend" => "Suspend",
                "resume" => "Resume",
                "refresh" => "Refresh",
                "open" => "Open",
                "close" => "Close",
                "close_clicked" => "CloseClicked",
                "refresh_clicked" => "RefreshClicked",
                "settings_clicked" => "SettingsClicked",
                "open_settings" => "OpenSettings",
                "close_panel" => "ClosePanel",
                "frame" => "Frame",
                "query" => "Query",
                "activate" => "Activate",
                "search" => "Search",
                "search_change" => "SearchChange",
                other => panic!("unmapped callback {other}"),
            };
            assert_eq!(
                variant_label(name),
                Some(callback.event),
                "variant {name}"
            );
        }
        assert_eq!(variant_label("Unknown"), None);
    }
}
