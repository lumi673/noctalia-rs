//! Typed callback and action handles.
//!
//! Both handles carry the type of the value they receive. The id comes from
//! that type, so plugins do not keep a parallel set of hand-written strings.
//! The compiler turns these handles into real Luau closures and plugin globals
//! in strict mode. Auto and off keep the bridge markers.

use std::any::type_name;
use std::borrow::Cow;
use std::fmt;
use std::marker::PhantomData;

use serde::de::{Deserialize, Deserializer, Error as _};
use serde::ser::{Serialize, SerializeMap, Serializer};
use serde_json::Value;

/// Marker key the bridge uses for a host callback.
pub const CALLBACK_KEY: &str = "__host_callback";
/// Marker key the bridge uses for a UI action.
pub const ACTION_KEY: &str = "__call";

/// The outer type name used as a stable id.
///
/// `type_name` includes the module path and generic arguments. The outer name
/// is the part both the library and the compiler can read from the same source
/// spelling, so a type such as `events::RowClicked` and
/// `crate::events::RowClicked<Value>` both produce `RowClicked`.
#[must_use]
pub fn type_id<T>() -> &'static str {
    let name = type_name::<T>();
    let mut depth = 0usize;
    for (index, character) in name.char_indices().rev() {
        match character {
            '>' | ']' | ')' => depth = depth.saturating_add(1),
            '<' | '[' | '(' => depth = depth.saturating_sub(1),
            ':' if depth == 0 && name[..index].ends_with(':') => {
                let tail = &name[index.saturating_add(1)..];
                return tail.split(['<', '[']).next().unwrap_or(tail);
            }
            _ => {}
        }
    }
    name.split(['<', '[']).next().unwrap_or(name)
}

/// A handle to the host callback body chosen by `T`.
///
/// The handle serializes to the same marker `ui::callback` writes, so auto and
/// off keep working. Strict emit reads the type at compile time and writes a
/// closure that forwards to that body.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Callback<T = Value> {
    id: Cow<'static, str>,
    marker: PhantomData<fn(T)>,
}

impl<T> Callback<T> {
    /// A handle whose id is the outer name of `T`.
    #[must_use]
    pub fn new() -> Self {
        Self::named(Cow::Borrowed(type_id::<T>()))
    }

    /// A handle with an explicit id, for plugins that need a fixed name.
    #[must_use]
    pub fn named(id: impl Into<Cow<'static, str>>) -> Self {
        Self {
            id: id.into(),
            marker: PhantomData,
        }
    }

    /// The stable id of this callback.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The bridge marker value.
    #[must_use]
    pub fn value(&self) -> Value {
        serde_json::json!({ CALLBACK_KEY: self.id })
    }
}

impl<T> Default for Callback<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Serialize for Callback<T> {
    fn serialize<S: Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(1))?;
        map.serialize_entry(CALLBACK_KEY, self.id.as_ref())?;
        map.end()
    }
}

impl<'de, T> Deserialize<'de> for Callback<T> {
    fn deserialize<D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        let id = value.get(CALLBACK_KEY).and_then(Value::as_str).ok_or_else(
            || D::Error::custom("expected a host callback marker"),
        )?;
        Ok(Self::named(id.to_owned()))
    }
}

/// A named UI action whose arguments have type `T`.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Action<T = Value> {
    name: Cow<'static, str>,
    marker: PhantomData<fn(T)>,
}

impl<T> Action<T> {
    /// An action whose name is the outer name of `T`.
    #[must_use]
    pub fn new() -> Self {
        Self::named(Cow::Borrowed(type_id::<T>()))
    }

    /// An action with an explicit name, for plugins that need a fixed name.
    #[must_use]
    pub fn named(name: impl Into<Cow<'static, str>>) -> Self {
        Self {
            name: name.into(),
            marker: PhantomData,
        }
    }

    /// The stable name of this action.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The bridge marker value.
    #[must_use]
    pub fn value(&self) -> Value {
        serde_json::json!({ ACTION_KEY: self.name })
    }
}

impl<T> Default for Action<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Serialize for Action<T> {
    fn serialize<S: Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(1))?;
        map.serialize_entry(ACTION_KEY, self.name.as_ref())?;
        map.end()
    }
}

impl<'de, T> Deserialize<'de> for Action<T> {
    fn deserialize<D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        let name = value
            .get(ACTION_KEY)
            .and_then(Value::as_str)
            .ok_or_else(|| D::Error::custom("expected a UI action marker"))?;
        Ok(Self::named(name.to_owned()))
    }
}

impl<T> fmt::Display for Callback<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.id.as_ref())
    }
}

impl<T> fmt::Display for Action<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name.as_ref())
    }
}

/// A callback handle for `T`, without an explicit id.
#[must_use]
pub fn callback<T>() -> Callback<T> {
    Callback::new()
}

/// An action handle for `T`, without an explicit name.
#[must_use]
pub fn action<T>() -> Action<T> {
    Action::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct RowClicked;
    struct Refresh;
    type Clicked = RowClicked;

    #[test]
    fn type_ids_use_the_outer_type_name() {
        assert_eq!(type_id::<RowClicked>(), "RowClicked");
        assert_eq!(type_id::<Option<RowClicked>>(), "Option");
        assert_eq!(Callback::<RowClicked>::new().id(), "RowClicked");
        assert_eq!(Action::<Refresh>::new().name(), "Refresh");
        assert_eq!(type_id::<&str>(), "&str");
        assert_eq!(type_id::<Clicked>(), "RowClicked");
    }

    #[test]
    fn handles_serialize_to_bridge_markers() {
        assert_eq!(
            serde_json::to_value(callback::<RowClicked>()).expect("callback"),
            serde_json::json!({ "__host_callback": "RowClicked" })
        );
        assert_eq!(
            serde_json::to_value(action::<Refresh>()).expect("action"),
            serde_json::json!({ "__call": "Refresh" })
        );
        assert_eq!(
            serde_json::to_value(Callback::<RowClicked>::named("row"))
                .expect("named callback"),
            serde_json::json!({ "__host_callback": "row" })
        );
    }

    #[test]
    fn handles_deserialize_only_their_own_marker() {
        let callback: Callback<RowClicked> = serde_json::from_value(
            serde_json::json!({ "__host_callback": "row" }),
        )
        .expect("callback");
        assert_eq!(callback.id(), "row");
        assert!(
            serde_json::from_value::<Action<Refresh>>(
                serde_json::json!({ "__host_callback": "row" })
            )
            .is_err()
        );
    }
}
