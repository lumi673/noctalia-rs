use std::borrow::Cow;

use serde::Serialize;
use serde_json::{Value, json};
#[derive(Clone, Debug, Serialize)]
pub struct UiNode {
    kind: Cow<'static, str>,
    props: Value,
    children: Vec<Self>,
}

#[inline]
#[must_use]
pub fn node(kind: &str, props: Value, children: Vec<UiNode>) -> UiNode {
    owned_node(kind.to_owned(), props, children)
}

#[inline]
fn owned_node(
    kind: impl Into<Cow<'static, str>>,
    props: Value,
    children: Vec<UiNode>,
) -> UiNode {
    UiNode {
        kind: kind.into(),
        props,
        children,
    }
}

#[inline]
#[must_use]
pub fn column(props: Value, children: Vec<UiNode>) -> UiNode {
    owned_node("column", props, children)
}

#[inline]
#[must_use]
pub fn row(props: Value, children: Vec<UiNode>) -> UiNode {
    owned_node("row", props, children)
}

#[inline]
#[must_use]
pub fn scroll(props: Value, children: Vec<UiNode>) -> UiNode {
    owned_node("scroll", props, children)
}

#[inline]
#[must_use]
pub fn label(props: Value) -> UiNode {
    owned_node("label", props, Vec::new())
}

#[inline]
#[must_use]
pub fn button(props: Value) -> UiNode {
    owned_node("button", props, Vec::new())
}

#[inline]
#[must_use]
pub fn input(props: Value) -> UiNode {
    owned_node("input", props, Vec::new())
}

#[inline]
#[must_use]
pub fn progress(props: Value) -> UiNode {
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
