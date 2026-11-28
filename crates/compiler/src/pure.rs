//! The strict-only allowlist of pure crate calls.
//!
//! Strict emit has no helper process, so a crate call can only run when its
//! result is expressible as Luau over the plugin API. Each call below has a
//! direct lowering with the same observable value. Anything else under an
//! allowlisted crate root fails at the call site with its source position.

use std::collections::BTreeSet;

/// One lowering that may be emitted into a strict script.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum Helper {
    HexEncode,
    HexDecode,
}

impl Helper {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::HexEncode => "__pure_encode_hex",
            Self::HexDecode => "__pure_decode_hex",
        }
    }

    pub(crate) fn source(self) -> &'static str {
        match self {
            Self::HexEncode => concat!(
                "local function __pure_encode_hex(__bytes)\n",
                "  local __digits = \"0123456789abcdef\"\n",
                "  local __out, __n = {}, #__bytes\n",
                "  for __i = 1, __n do\n",
                "    local __byte = string.byte(__bytes, __i)\n",
                "    local __high = math.floor(__byte / 16) + 1\n",
                "    local __low = (__byte % 16) + 1\n",
                "    __out[#__out + 1] = string.sub(__digits, __high, __high)\n",
                "    __out[#__out + 1] = string.sub(__digits, __low, __low)\n",
                "  end\n",
                "  return table.concat(__out)\n",
                "end\n"
            ),
            Self::HexDecode => concat!(
                "local function __pure_hex_value(__byte)\n",
                "  if __byte >= 48 and __byte <= 57 then return __byte - 48 end\n",
                "  if __byte >= 97 and __byte <= 102 then return __byte - 87 end\n",
                "  if __byte >= 65 and __byte <= 70 then return __byte - 55 end\n",
                "  return nil\n",
                "end\n",
                "local function __pure_decode_hex(__text)\n",
                "  local __out, __n, __i = {}, #__text, 1\n",
                "  while __i + 1 <= __n do\n",
                "    local __high = __pure_hex_value(string.byte(__text, __i))\n",
                "    local __low = __pure_hex_value(string.byte(__text, __i + 1))\n",
                "    if __high and __low then\n",
                "      __out[#__out + 1] = string.char(__high * 16 + __low)\n",
                "    end\n",
                "    __i += 2\n",
                "  end\n",
                "  return table.concat(__out)\n",
                "end\n"
            ),
        }
    }
}

/// How an allowed call lowers to Luau.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Lowering {
    /// The value already has the Luau shape Rust serializes to.
    Identity,
    JsonEncode,
    JsonDecode,
    EmptyMap,
    Helper(Helper),
}

/// One allowed call and the number of arguments it takes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Call {
    pub(crate) lowering: Lowering,
    pub(crate) arity: usize,
}

static CALLS: &[(&str, Call)] = &[
    (
        "serde_json::to_vec",
        Call {
            lowering: Lowering::JsonEncode,
            arity: 1,
        },
    ),
    (
        "serde_json::from_slice",
        Call {
            lowering: Lowering::JsonDecode,
            arity: 1,
        },
    ),
    (
        "serde_json::to_value",
        Call {
            lowering: Lowering::Identity,
            arity: 1,
        },
    ),
    (
        "serde_json::Map::new",
        Call {
            lowering: Lowering::EmptyMap,
            arity: 0,
        },
    ),
    (
        "noctalia_rs::bytes::encode_hex",
        Call {
            lowering: Lowering::Helper(Helper::HexEncode),
            arity: 1,
        },
    ),
    (
        "noctalia_rs::bytes::decode_hex",
        Call {
            lowering: Lowering::Helper(Helper::HexDecode),
            arity: 1,
        },
    ),
];

/// Crate roots covered by this lowering table. Other roots keep the compiler's
/// existing strict checks; serde derives and std time reads need no new call
/// lowering.
static ROOTS: &[&str] = &["serde_json", "noctalia_rs::bytes"];

pub(crate) fn call(name: &str) -> Option<Call> {
    CALLS
        .iter()
        .find(|(path, _)| *path == name)
        .map(|(_, call)| *call)
}

/// Whether a name belongs to a crate in the allowlist.
pub(crate) fn allowlisted(name: &str) -> bool {
    ROOTS
        .iter()
        .any(|root| name == *root || name.starts_with(&format!("{root}::")))
}

/// Helpers used by the calls in `used`, in emission order.
pub(crate) fn helpers(used: &BTreeSet<Helper>) -> String {
    used.iter()
        .map(|helper| helper.source())
        .collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_covers_exact_paths_only() {
        assert!(allowlisted("serde_json::to_string_pretty"));
        assert!(allowlisted("noctalia_rs::bytes::decode_hex"));
        assert!(!allowlisted("serde_jsonish::to_vec"));
        assert!(!allowlisted("local::to_vec"));
        assert!(call("serde_json::to_vec").is_some());
        assert!(call("serde_json::to_string_pretty").is_none());
    }

    #[test]
    fn hex_helpers_have_distinct_names() {
        assert_ne!(Helper::HexEncode.name(), Helper::HexDecode.name());
    }
}
