//! Declarative description of the host API exposed by noctalia-rs.
//!
//! The entries mirror the official `noctalia.d.luau` definitions plus the
//! capabilities introduced by plugin API 54 through 57. The parity tests
//! compare this table with the official surface snapshot.

/// How a Luau call is delivered to the host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemberKind {
    /// Fire and forget. Emits a host call and returns nothing.
    Void,
    /// Emits a host call and reports acceptance.
    Bool,
    /// Queries the host and returns its value.
    Value,
    /// Queries the host and returns `(value, err)`.
    Pair,
    /// Implemented directly by the embedded runtime.
    Handler,
    /// A namespace table.
    Namespace,
    /// A declarative UI constructor.
    Node,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Member {
    pub root: &'static str,
    pub name: &'static str,
    pub kind: MemberKind,
}

macro_rules! members {
    ($root:expr, $($name:literal => $kind:ident),* $(,)?) => {
        &[$(Member { root: $root, name: $name, kind: MemberKind::$kind }),*]
    };
}

/// Everything the host injects into a plugin script.
static GROUPS: &[&[Member]] = &[
    members!(
        "noctalia",
        "log" => Handler,
        "runAsync" => Handler,
        "runStream" => Handler,
        "runInTerminal" => Bool,
        "commandExists" => Value,
        "processMatches" => Value,
        "flatpakAppInstalled" => Value,
        "portalAvailable" => Value,
        "focusedOutputName" => Value,
        "outputs" => Value,
        "isDarkMode" => Handler,
        "getColor" => Handler,
        "getSetting" => Handler,
        "appIconPath" => Value,
        "setWallpaperEnabled" => Void,
        "setWallpaper" => Handler,
        "wallpaperDirectory" => Value,
        "wallpaperPath" => Value,
        "setWallpaperMask" => Void,
        "togglePanel" => Handler,
        "openSettings" => Handler,
        "openColorPicker" => Value,
        "notify" => Handler,
        "notifyError" => Handler,
        "openPath" => Handler,
        "revealPath" => Void,
        "copyToClipboard" => Handler,
        "clipboardText" => Value,
        "calendarEvents" => Value,
        "mediaPlayer" => Value,
        "getenv" => Value,
        "expandPath" => Value,
        "formatTime" => Value,
        "timeFormat" => Value,
        "dateFormat" => Value,
        "isValidTimezone" => Value,
        "nowMs" => Handler,
        "systemStats" => Value,
        "setGpuMonitoring" => Value,
        "cpuCores" => Value,
        "diskMounts" => Value,
        "diskStats" => Value,
        "setUpdateInterval" => Handler,
        "readFile" => Value,
        "readFileAsync" => Value,
        "writeFile" => Pair,
        "mkdirAll" => Pair,
        "removeFile" => Pair,
        "renameFile" => Pair,
        "fileExists" => Value,
        "fileInfo" => Value,
        "listDir" => Pair,
        "pluginDir" => Value,
        "pluginDataDir" => Pair,
        "loadFont" => Pair,
        "tr" => Value,
        "trp" => Value,
        "http" => Bool,
        "httpStream" => Handler,
        "download" => Value,
        "fuzzyScore" => Value,
        "getConfig" => Handler,
        "state" => Namespace,
        "sound" => Namespace,
        "idle" => Namespace,
        "nightlight" => Namespace,
        "layer" => Namespace,
        "umbriel" => Namespace,
        "compositor" => Namespace,
        "osd" => Namespace,
        "json" => Namespace,
        "string" => Namespace,
    ),
    members!(
        "noctalia.state",
        "set" => Void,
        "get" => Handler,
        "watch" => Handler,
    ),
    members!("noctalia.json", "decode" => Handler, "encode" => Handler),
    members!(
        "noctalia.string",
        "trim" => Handler,
        "urlEncode" => Handler,
        "urlDecode" => Handler,
    ),
    members!(
        "noctalia.sound",
        "load" => Value,
        "play" => Void,
    ),
    members!(
        "noctalia.idle",
        "watch" => Value,
        "unwatch" => Value,
    ),
    members!(
        "noctalia.nightlight",
        "state" => Value,
        "setEnabled" => Void,
        "toggle" => Void,
        "setForce" => Void,
        "toggleForce" => Void,
        "watch" => Value,
        "unwatch" => Value,
    ),
    members!("noctalia.umbriel", "action" => Value),
    members!(
        "noctalia.compositor",
        "name" => Value,
        "subscribe" => Value,
        "unsubscribe" => Value,
    ),
    members!(
        "noctalia.layer",
        "show" => Handler,
        "hide" => Handler,
        "configure" => Handler,
    ),
    members!("noctalia.osd", "show" => Handler),
    members!(
        "ui",
        "column" => Node,
        "row" => Node,
        "scroll" => Node,
        "dragSource" => Node,
        "dropZone" => Node,
        "box" => Node,
        "label" => Node,
        "fillLabel" => Node,
        "markdown" => Node,
        "glyph" => Node,
        "image" => Node,
        "pixelStream" => Node,
        "colorPicker" => Node,
        "separator" => Node,
        "spacer" => Node,
        "progress" => Node,
        "button" => Node,
        "graph" => Node,
        "input" => Node,
        "select" => Node,
        "slider" => Node,
        "toggle" => Node,
        "ring" => Node,
    ),
    members!(
        "barWidget",
        "setText" => Handler,
        "setGlyph" => Void,
        "setImage" => Handler,
        "setTooltip" => Handler,
        "clearTooltip" => Handler,
        "setFont" => Void,
        "setColor" => Handler,
        "setGlyphColor" => Void,
        "isVertical" => Handler,
        "outputName" => Handler,
        "setVisible" => Handler,
        "render" => Handler,
    ),
    members!(
        "shortcut",
        "setLabel" => Void,
        "setIcon" => Void,
        "setActive" => Void,
        "setEnabled" => Void,
    ),
    members!("launcher", "setResults" => Handler, "setQuery" => Void),
    members!(
        "desktopWidget",
        "render" => Handler,
        "setWantsSecondTicks" => Void,
        "setNeedsFrameTick" => Void,
        "setVisible" => Void,
    ),
    members!(
        "panel",
        "render" => Handler,
        "close" => Void,
        "openContextMenu" => Value,
        "setWantsSecondTicks" => Void,
        "setNeedsFrameTick" => Void,
    ),
    members!(
        "layer",
        "render" => Handler,
        "setNeedsFrameTick" => Void,
        "outputName" => Value,
    ),
];

/// Iterates over every registered API member.
pub fn members() -> impl Iterator<Item = &'static Member> {
    GROUPS.iter().flat_map(|group| group.iter())
}

/// Returns the registered member, if any.
#[must_use]
pub fn find(root: &str, name: &str) -> Option<&'static Member> {
    members().find(|member| member.root == root && member.name == name)
}

/// Returns all members registered below a root namespace.
pub fn root(root: &str) -> impl Iterator<Item = &'static Member> {
    members().filter(move |member| member.root == root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn members_have_unique_paths() {
        let mut paths = BTreeSet::new();
        for member in members() {
            let path = format!("{}.{}", member.root, member.name);
            assert!(paths.insert(path.clone()), "duplicate member {path}");
        }
    }

    #[test]
    fn find_returns_nested_members() {
        assert_eq!(
            find("noctalia.idle", "watch").map(|member| member.kind),
            Some(MemberKind::Value)
        );
        assert!(find("noctalia", "idle.watch").is_none());
    }
}
