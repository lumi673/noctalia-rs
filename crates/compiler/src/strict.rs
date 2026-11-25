//! Strict-mode data and the plugin API table the emitter resolves against.
//!
//! Strict emit has no bridge and no helper. A host call must therefore be the
//! direct plugin API member, and the parts the bridge assembled at run time
//! come from the build instead.

/// How a plugin API 57 member delivers a call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemberKind {
    /// Fire and forget, returns nothing.
    Void,
    /// Reports acceptance.
    Bool,
    /// Returns the host value.
    Value,
    /// Returns `(value, error)`.
    Pair,
    /// Implemented by the embedded runtime.
    Handler,
    /// A namespace table.
    Namespace,
    /// A declarative UI constructor.
    Node,
}

/// What a strict entry receives from the build.
///
/// `assets` is every declared asset with the mode the build gives it.
#[derive(Clone, Copy)]
pub struct StrictOptions<'a> {
    /// `local CONFIG = {...}` from the declared manifest keys.
    pub config: &'a str,
    /// `LANGUAGES` and `TRANSLATIONS` from `translations/*.json`.
    pub translations: &'a str,
    pub assets: &'a [(&'a str, u32)],
}

struct Group {
    root: &'static str,
    members: &'static [(&'static str, MemberKind)],
}

use MemberKind::{Bool, Handler, Namespace, Node, Pair, Value, Void};

/// The plugin API 57 surface, mirrored from `noctalia_rs::api`.
///
/// A test in `crates/luau` compares this table with the real one.
static GROUPS: &[Group] = &[
    Group {
        root: "noctalia",
        members: &[
            ("log", Handler),
            ("runAsync", Handler),
            ("runStream", Handler),
            ("runInTerminal", Bool),
            ("commandExists", Value),
            ("processMatches", Value),
            ("flatpakAppInstalled", Value),
            ("portalAvailable", Value),
            ("focusedOutputName", Value),
            ("outputs", Value),
            ("isDarkMode", Handler),
            ("getColor", Handler),
            ("getSetting", Handler),
            ("appIconPath", Value),
            ("setWallpaperEnabled", Void),
            ("setWallpaper", Handler),
            ("wallpaperDirectory", Value),
            ("wallpaperPath", Value),
            ("setWallpaperMask", Void),
            ("togglePanel", Handler),
            ("openSettings", Handler),
            ("openColorPicker", Value),
            ("notify", Handler),
            ("notifyError", Handler),
            ("openPath", Handler),
            ("revealPath", Void),
            ("copyToClipboard", Handler),
            ("clipboardText", Value),
            ("calendarEvents", Value),
            ("mediaPlayer", Value),
            ("getenv", Value),
            ("expandPath", Value),
            ("formatTime", Value),
            ("timeFormat", Value),
            ("dateFormat", Value),
            ("isValidTimezone", Value),
            ("nowMs", Handler),
            ("systemStats", Value),
            ("setGpuMonitoring", Value),
            ("cpuCores", Value),
            ("diskMounts", Value),
            ("diskStats", Value),
            ("setUpdateInterval", Handler),
            ("readFile", Value),
            ("readFileAsync", Value),
            ("writeFile", Pair),
            ("mkdirAll", Pair),
            ("removeFile", Pair),
            ("renameFile", Pair),
            ("fileExists", Value),
            ("fileInfo", Value),
            ("listDir", Pair),
            ("pluginDir", Value),
            ("pluginDataDir", Pair),
            ("loadFont", Pair),
            ("tr", Value),
            ("trp", Value),
            ("http", Bool),
            ("httpStream", Handler),
            ("download", Value),
            ("fuzzyScore", Value),
            ("getConfig", Handler),
            ("state", Namespace),
            ("sound", Namespace),
            ("idle", Namespace),
            ("nightlight", Namespace),
            ("layer", Namespace),
            ("umbriel", Namespace),
            ("compositor", Namespace),
            ("osd", Namespace),
            ("json", Namespace),
            ("string", Namespace),
        ],
    },
    Group {
        root: "noctalia.state",
        members: &[("set", Void), ("get", Handler), ("watch", Handler)],
    },
    Group {
        root: "noctalia.json",
        members: &[("decode", Handler), ("encode", Handler)],
    },
    Group {
        root: "noctalia.string",
        members: &[
            ("trim", Handler),
            ("urlEncode", Handler),
            ("urlDecode", Handler),
        ],
    },
    Group {
        root: "noctalia.sound",
        members: &[("load", Value), ("play", Void)],
    },
    Group {
        root: "noctalia.idle",
        members: &[("watch", Value), ("unwatch", Value)],
    },
    Group {
        root: "noctalia.nightlight",
        members: &[
            ("state", Value),
            ("setEnabled", Void),
            ("toggle", Void),
            ("setForce", Void),
            ("toggleForce", Void),
            ("watch", Value),
            ("unwatch", Value),
        ],
    },
    Group {
        root: "noctalia.umbriel",
        members: &[("action", Value)],
    },
    Group {
        root: "noctalia.compositor",
        members: &[
            ("name", Value),
            ("subscribe", Value),
            ("unsubscribe", Value),
        ],
    },
    Group {
        root: "noctalia.layer",
        members: &[
            ("show", Handler),
            ("hide", Handler),
            ("configure", Handler),
        ],
    },
    Group {
        root: "noctalia.osd",
        members: &[("show", Handler)],
    },
    Group {
        root: "ui",
        members: &[
            ("column", Node),
            ("row", Node),
            ("scroll", Node),
            ("dragSource", Node),
            ("dropZone", Node),
            ("box", Node),
            ("label", Node),
            ("fillLabel", Node),
            ("markdown", Node),
            ("glyph", Node),
            ("image", Node),
            ("pixelStream", Node),
            ("colorPicker", Node),
            ("separator", Node),
            ("spacer", Node),
            ("progress", Node),
            ("button", Node),
            ("graph", Node),
            ("input", Node),
            ("select", Node),
            ("slider", Node),
            ("toggle", Node),
            ("ring", Node),
        ],
    },
    Group {
        root: "barWidget",
        members: &[
            ("setText", Handler),
            ("setGlyph", Void),
            ("setImage", Handler),
            ("setTooltip", Handler),
            ("clearTooltip", Handler),
            ("setFont", Void),
            ("setColor", Handler),
            ("setGlyphColor", Void),
            ("isVertical", Handler),
            ("outputName", Handler),
            ("setVisible", Handler),
            ("render", Handler),
        ],
    },
    Group {
        root: "shortcut",
        members: &[
            ("setLabel", Void),
            ("setIcon", Void),
            ("setActive", Void),
            ("setEnabled", Void),
        ],
    },
    Group {
        root: "launcher",
        members: &[("setResults", Handler), ("setQuery", Void)],
    },
    Group {
        root: "desktopWidget",
        members: &[
            ("render", Handler),
            ("setWantsSecondTicks", Void),
            ("setNeedsFrameTick", Void),
            ("setVisible", Void),
        ],
    },
    Group {
        root: "panel",
        members: &[
            ("render", Handler),
            ("close", Void),
            ("openContextMenu", Value),
            ("setWantsSecondTicks", Void),
            ("setNeedsFrameTick", Void),
        ],
    },
    Group {
        root: "layer",
        members: &[
            ("render", Handler),
            ("setNeedsFrameTick", Void),
            ("outputName", Value),
        ],
    },
];

/// One API namespace and its members.
pub type MemberTable = (&'static str, &'static [(&'static str, MemberKind)]);

/// Every group, for the parity test.
pub fn groups() -> impl Iterator<Item = MemberTable> {
    GROUPS.iter().map(|group| (group.root, group.members))
}

/// Resolves one member, given the query root and the rest of the path.
pub fn member(root: &str, path: &[String]) -> Option<MemberKind> {
    let (name, namespace) = path.split_last()?;
    let mut full = root.to_owned();
    for part in namespace {
        full.push('.');
        full.push_str(part);
    }
    let group = GROUPS.iter().find(|group| group.root == full)?;
    group
        .members
        .iter()
        .find(|(member, _)| member == &name.as_str())
        .map(|(_, kind)| *kind)
}

/// The direct Luau call path for a member, such as `noctalia.getSetting`.
pub fn member_path(root: &str, path: &[String]) -> String {
    let mut out = root.to_owned();
    for part in path {
        out.push('.');
        out.push_str(part);
    }
    out
}

/// Global that receives a host callback by id.
pub(crate) const CALLBACK_DISPATCH: &str = "__strict_callback";

/// Global that receives an action by name.
pub(crate) const ACTION_DISPATCH: &str = "__strict_action";

/// The plugin global emitted for one `ui::call(name)`.
///
/// The name is sanitized for Luau and carries a hash of the original name, so
/// two names that sanitize alike stay distinct and body emission order does
/// not matter.
pub fn action_global(name: &str) -> String {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in name.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    format!("__strict_action_{}_{hash:08x}", crate::sanitize(name))
}

/// A closure that forwards its arguments to a dispatcher global.
///
/// The id is bound once as an IIFE parameter, so a local the caller later
/// reassigns is captured at registration, the way Rust and the bridge capture
/// their callback name.
pub fn closure(dispatcher: &str, id: &str) -> String {
    format!(
        "(function(__captured) return function(...) {dispatcher}(__captured, ...) end end)({id})"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn members_resolve_nested_roots() {
        assert_eq!(
            member("noctalia", &["getSetting".to_owned()]),
            Some(Handler)
        );
        assert_eq!(
            member("noctalia", &["state".to_owned(), "watch".to_owned()]),
            Some(Handler)
        );
        assert_eq!(
            member("panel", &["openContextMenu".to_owned()]),
            Some(Value)
        );
        assert_eq!(member("noctalia", &["missing".to_owned()]), None);
        assert_eq!(member("nope", &["getSetting".to_owned()]), None);
    }

    #[test]
    fn action_globals_are_sanitized_and_distinct() {
        assert_eq!(
            action_global("onTrackHover"),
            "__strict_action_onTrackHover_2dba7e07"
        );
        assert_ne!(action_global("a-b"), action_global("a_b"));
        assert!(action_global("1 2").starts_with("__strict_action_1_2_"));
    }

    #[test]
    fn member_paths_are_dotted() {
        assert_eq!(
            member_path("noctalia", &["state".to_owned(), "watch".to_owned()]),
            "noctalia.state.watch"
        );
    }
}
