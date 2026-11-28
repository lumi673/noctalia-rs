use std::borrow::Cow;

use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};
use serde_json::{Value, json};
use smallvec::SmallVec;

type Entries = SmallVec<[(&'static str, Prop); 2]>;

/// One property value in a declarative UI node.
#[derive(Clone, Debug)]
enum Prop {
    Text(Cow<'static, str>),
    Integer(i64),
    Number(f64),
    Boolean(bool),
    Call(Cow<'static, str>),
    Json(Value),
    HostCallback(String),
}

impl Serialize for Prop {
    fn serialize<S: Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match self {
            Self::Text(value) => serializer.serialize_str(value),
            Self::Integer(value) => serializer.serialize_i64(*value),
            Self::Number(value) => serializer.serialize_f64(*value),
            Self::Boolean(value) => serializer.serialize_bool(*value),
            Self::Call(name) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("__call", name.as_ref())?;
                map.end()
            }
            Self::HostCallback(id) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("__host_callback", id)?;
                map.end()
            }
            Self::Json(value) => value.serialize(serializer),
        }
    }
}

/// A property list for a declarative UI node.
///
/// [`json!`] builds a `serde_json::Map` per node, which allocates one
/// [`String`] per key and another for every string value. This form keeps keys
/// and static values borrowed, and two properties fit inline.
///
/// The object carries the same keys and values as the [`json!`] form, in
/// insertion order rather than sorted.
///
/// ```
/// use noctalia_rs::{Props, ui};
///
/// let node = ui::label(
///     Props::new()
///         .text("text", "Hello")
///         .text("color", "primary")
///         .int("fontSize", 12),
/// );
/// # let value = serde_json::to_value(node).unwrap();
/// # assert_eq!(value["props"]["color"], "primary");
/// ```
#[derive(Clone, Debug, Default)]
pub struct Props(Entries);

impl Props {
    /// An empty property list.
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self(SmallVec::new())
    }

    /// Adds a string, borrowing the value when it is already static.
    #[inline]
    #[must_use]
    pub fn text(
        mut self,
        key: &'static str,
        value: impl Into<Cow<'static, str>>,
    ) -> Self {
        self.0.push((key, Prop::Text(value.into())));
        self
    }

    /// Adds an integer, serialized without a decimal point.
    #[inline]
    #[must_use]
    pub fn int(mut self, key: &'static str, value: i64) -> Self {
        self.0.push((key, Prop::Integer(value)));
        self
    }

    /// Adds a floating point number.
    #[inline]
    #[must_use]
    pub fn num(mut self, key: &'static str, value: f64) -> Self {
        self.0.push((key, Prop::Number(value)));
        self
    }

    /// Adds a boolean.
    #[inline]
    #[must_use]
    pub fn bool(mut self, key: &'static str, value: bool) -> Self {
        self.0.push((key, Prop::Boolean(value)));
        self
    }

    /// Adds a UI action callback, the same marker [`call`] builds.
    #[inline]
    #[must_use]
    pub fn call(mut self, key: &'static str, name: &'static str) -> Self {
        self.0.push((key, Prop::Call(Cow::Borrowed(name))));
        self
    }

    /// Adds a typed UI action handler.
    #[inline]
    #[must_use]
    pub fn action(mut self, key: &'static str, action: Action) -> Self {
        self.0.push((key, Prop::Call(action.0)));
        self
    }

    /// Adds a typed host callback handle.
    #[inline]
    #[must_use]
    pub fn callback(mut self, key: &'static str, id: CallbackId) -> Self {
        self.0.push((key, Prop::HostCallback(id.0)));
        self
    }

    /// Adds any other JSON value.
    #[inline]
    #[must_use]
    pub fn json(mut self, key: &'static str, value: Value) -> Self {
        self.0.push((key, Prop::Json(value)));
        self
    }
}

/// A stable UI action name passed to a typed prop handler.
///
/// In strict mode the compiler gives each action its own Luau global. In auto
/// and off modes it serializes to the same `__call` marker as [`call`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Action(Cow<'static, str>);

impl Action {
    /// Declares an action by its stable name.
    #[inline]
    #[must_use]
    pub const fn new(name: &'static str) -> Self {
        Self(Cow::Borrowed(name))
    }

    /// Declares an action from a computed name.
    #[inline]
    #[must_use]
    pub fn dynamic(name: impl Into<String>) -> Self {
        Self(Cow::Owned(name.into()))
    }

    /// The name the host dispatches.
    #[inline]
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A host callback handle for a typed prop handler.
///
/// In strict mode the compiler emits a real Luau closure for it. In auto and
/// off modes it serializes to the same `__host_callback` marker as [`callback`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallbackId(String);

impl CallbackId {
    /// Wraps a stable callback id.
    #[inline]
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The id the host dispatches.
    #[inline]
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Serialize for Props {
    fn serialize<S: Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (key, value) in &self.0 {
            map.serialize_entry(*key, value)?;
        }
        map.end()
    }
}

/// The stored property form of a declarative UI node.
///
/// Constructors accept anything that converts into it, so both
/// [`Props`] and a [`serde_json::Value`] work:
///
/// ```
/// use noctalia_rs::{Props, ui};
/// use serde_json::json;
///
/// let _typed = ui::label(Props::new().text("text", "typed"));
/// let _untyped = ui::label(json!({ "text": "json" }));
/// ```
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum NodeProps {
    /// Borrowed, allocation-light properties from [`Props`].
    Typed(Props),
    /// A `serde_json` object, kept as-is for compatibility.
    Value(Value),
}

impl Serialize for NodeProps {
    fn serialize<S: Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match self {
            Self::Typed(props) => props.serialize(serializer),
            Self::Value(value) => value.serialize(serializer),
        }
    }
}

impl From<Props> for NodeProps {
    #[inline]
    fn from(props: Props) -> Self {
        Self::Typed(props)
    }
}

impl From<Value> for NodeProps {
    #[inline]
    fn from(value: Value) -> Self {
        Self::Value(value)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct UiNode {
    kind: Cow<'static, str>,
    props: NodeProps,
    children: Vec<Self>,
}

#[inline]
#[must_use]
pub fn node(
    kind: &str,
    props: impl Into<NodeProps>,
    children: Vec<UiNode>,
) -> UiNode {
    owned_node(kind.to_owned(), props, children)
}

#[inline]
fn owned_node(
    kind: impl Into<Cow<'static, str>>,
    props: impl Into<NodeProps>,
    children: Vec<UiNode>,
) -> UiNode {
    UiNode {
        kind: kind.into(),
        props: props.into(),
        children,
    }
}

#[inline]
#[must_use]
pub fn column(props: impl Into<NodeProps>, children: Vec<UiNode>) -> UiNode {
    owned_node("column", props, children)
}

#[inline]
#[must_use]
pub fn row(props: impl Into<NodeProps>, children: Vec<UiNode>) -> UiNode {
    owned_node("row", props, children)
}

#[inline]
#[must_use]
pub fn scroll(props: impl Into<NodeProps>, children: Vec<UiNode>) -> UiNode {
    owned_node("scroll", props, children)
}

#[inline]
#[must_use]
pub fn label(props: impl Into<NodeProps>) -> UiNode {
    owned_node("label", props, Vec::new())
}

#[inline]
#[must_use]
pub fn button(props: impl Into<NodeProps>) -> UiNode {
    owned_node("button", props, Vec::new())
}

#[inline]
#[must_use]
pub fn input(props: impl Into<NodeProps>) -> UiNode {
    owned_node("input", props, Vec::new())
}

#[inline]
#[must_use]
pub fn progress(props: impl Into<NodeProps>) -> UiNode {
    owned_node("progress", props, Vec::new())
}

#[inline]
#[must_use]
pub fn call(name: &str) -> Value {
    json!({ "__call": name })
}

#[inline]
#[must_use]
pub fn callback(id: &str) -> Value {
    json!({ "__host_callback": id })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_serializes_kind_props_and_children() {
        let tree = node(
            "column",
            json!({ "gap": 4 }),
            vec![label(json!({ "text": "hi" }))],
        );

        let value = serde_json::to_value(tree).expect("serialize");

        assert_eq!(value["kind"], "column");
        assert_eq!(value["children"][0]["kind"], "label");
    }

    #[test]
    fn test_builders_use_expected_kinds() {
        let kinds = vec![
            row(json!({}), Vec::new()).serialize_kind(),
            scroll(json!({}), Vec::new()).serialize_kind(),
            button(json!({})).serialize_kind(),
            input(json!({})).serialize_kind(),
            progress(json!({})).serialize_kind(),
        ];

        assert_eq!(kinds, vec!["row", "scroll", "button", "input", "progress"]);
    }

    #[test]
    fn test_calls_encode_markers() {
        assert_eq!(call("save"), json!({ "__call": "save" }));
        assert_eq!(callback("done"), json!({ "__host_callback": "done" }));
    }

    #[test]
    fn test_typed_handlers_match_the_marker_forms() {
        let action = Props::new().action("onClick", Action::new("save"));
        let callback_props =
            Props::new().callback("onHover", CallbackId::new("row-1"));

        assert_eq!(
            serde_json::to_value(action).expect("serialize action"),
            serde_json::to_value(Props::new().call("onClick", "save"))
                .expect("serialize call")
        );
        assert_eq!(
            serde_json::to_value(callback_props).expect("serialize callback"),
            serde_json::to_value(
                Props::new().json("onHover", callback("row-1"))
            )
            .expect("serialize marker")
        );
    }

    #[test]
    fn test_typed_props_match_the_json_form() {
        let typed = ui_node(
            Props::new()
                .text("text", "10m")
                .text("color", "on_surface_variant")
                .int("maxLines", 1)
                .num("progress", 0.5)
                .bool("focus", true)
                .call("onClick", "start")
                .json("key", json!(["a", 1])),
        );
        let untyped = ui_node(json!({
            "text": "10m",
            "color": "on_surface_variant",
            "maxLines": 1,
            "progress": 0.5,
            "focus": true,
            "onClick": { "__call": "start" },
            "key": ["a", 1],
        }));

        assert_eq!(
            serde_json::to_value(typed).expect("typed"),
            serde_json::to_value(untyped).expect("untyped")
        );
    }

    #[test]
    fn test_props_grow_past_inline_capacity() {
        let inline = Props::new().text("a", "1").text("b", "2");
        assert_eq!(
            serde_json::to_value(inline).expect("serialize"),
            json!({ "a": "1", "b": "2" })
        );

        let spilled = Props::new()
            .text("a", "1")
            .text("b", "2")
            .text("c", "3")
            .text("d", "4")
            .text("e", "5");
        assert_eq!(
            serde_json::to_value(spilled).expect("serialize"),
            json!({ "a": "1", "b": "2", "c": "3", "d": "4", "e": "5" })
        );

        assert_eq!(
            serde_json::to_value(Props::new()).expect("empty"),
            json!({})
        );
    }

    #[test]
    fn test_owned_text_is_kept() {
        let title = String::from("standup");
        let tree = column(
            Props::new().text("key", title.clone()),
            vec![label(Props::new().text("text", title))],
        );

        let value = serde_json::to_value(tree).expect("serialize");
        assert_eq!(value["props"]["key"], "standup");
        assert_eq!(value["children"][0]["props"]["text"], "standup");
    }

    fn ui_node(props: impl Into<NodeProps>) -> UiNode {
        label(props)
    }

    trait SerializeKind {
        fn serialize_kind(&self) -> String;
    }

    impl SerializeKind for UiNode {
        fn serialize_kind(&self) -> String {
            serde_json::to_value(self)
                .expect("serialize")
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        }
    }
}
