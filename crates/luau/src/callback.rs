use serde_json::Value;

use crate::host::Snapshot;

pub fn callback_for_event(event: &str) -> Option<&'static str> {
    match event {
        "update" | "render" | "init" => Some("update"),
        "click" => Some("onClick"),
        "right_click" => Some("onRightClick"),
        "middle_click" => Some("onMiddleClick"),
        "double_click" => Some("onDoubleClick"),
        "audio_spectrum" => Some("onAudioSpectrum"),
        "scroll" => Some("onScroll"),
        "hover" => Some("onHover"),
        "key" => Some("onKey"),
        "ipc" => Some("onIpc"),
        "config_changed" => Some("onConfigChanged"),
        "outputs_changed" => Some("onOutputsChanged"),
        "open" => Some("onOpen"),
        "close" => Some("onClose"),
        "close_clicked" => Some("onCloseClicked"),
        "refresh_clicked" => Some("onRefreshClicked"),
        "settings_clicked" => Some("onSettingsClicked"),
        "open_settings" => Some("onOpenSettings"),
        "close_panel" => Some("onClosePanel"),
        "activate" => Some("onActivate"),
        "query" => Some("onQuery"),
        "frame" | "frame_tick" => Some("onFrameTick"),
        "enable" => Some("onEnable"),
        "disable" => Some("onDisable"),
        "exit" => Some("onExit"),
        "suspend" => Some("onSuspend"),
        "resume" => Some("onResume"),
        "refresh" => Some("onRefresh"),
        "search" => Some("onSearch"),
        "search_change" => Some("onSearchChange"),
        _ => None,
    }
}

pub fn callback_args(snapshot: &Snapshot, callback: &str) -> Vec<Value> {
    let payload = snapshot.payload();
    if let Some(Value::Array(args)) = payload.get("args") {
        return args.clone();
    }
    if let Value::Array(args) = &payload {
        return args.clone();
    }

    let names: &[&str] = match callback {
        "onScroll" => &["axis", "steps", "starts_gesture", "startsGesture"],
        "onKey" => &["chord", "key", "pressed", "repeated", "modifiers"],
        "onAudioSpectrum" => &["values_csv", "state_csv", "valuesCsv"],
        "onIpc" => &["event", "payload"],
        "onConfigChanged" => &["key", "value"],
        "onOpen" => &["context"],
        "onActivate" => &["id"],
        "onQuery" | "onSearch" | "onSearchChange" => &["text", "query"],
        "onFrameTick" => &["delta", "delta_ms", "deltaMs"],
        "onHover" => &["hovered", "entered"],
        "onExit" => &["signal", "reason"],
        _ => &[],
    };

    let mut result = Vec::new();
    for name in names {
        if let Some(value) = payload.get(name) {
            result.push(value.clone());
        }
    }
    if !result.is_empty() {
        return result;
    }
    if matches!(callback, "onOpen") && payload.is_object() {
        return vec![payload.clone()];
    }
    Vec::new()
}
