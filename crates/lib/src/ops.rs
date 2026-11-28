use crate::bytes::ByteBuf;
use crate::ui::{UiNode, callback};
use serde::{Serialize, Serializer};
use serde_json::Value;
#[derive(Clone, Copy, Debug)]
pub struct SharedStr(&'static str);

impl Serialize for SharedStr {
    #[inline]
    fn serialize<S: Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SharedValue(&'static Value);

impl Serialize for SharedValue {
    #[inline]
    fn serialize<S: Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

#[doc(hidden)]
#[must_use]
pub fn parse_json_static(json: &'static str) -> Value {
    serde_json::from_str(json).unwrap_or_default()
}

#[inline]
fn bytes_value(bytes: ByteBuf) -> Value {
    serde_json::to_value(bytes).unwrap_or(Value::Null)
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Log {
        message: String,
    },
    #[serde(rename = "log")]
    LogShared {
        message: SharedStr,
    },
    Notify {
        title: String,
        body: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        options: Option<Value>,
    },
    NotifyError {
        title: String,
        body: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        options: Option<Value>,
    },
    TimerStart {
        handle: u64,
        ms: u64,
        callback: Value,
        #[serde(rename = "recurring")]
        repeat: bool,
    },
    TimerCancel {
        handle: u64,
    },
    SetInterval {
        ms: u64,
    },
    StateSet {
        key: String,
        value: Value,
    },
    #[serde(rename = "state_set")]
    StateSetShared {
        key: SharedStr,
        value: SharedValue,
    },
    TogglePanel {
        id: String,
    },
    OpenSettings,
    BarText {
        text: String,
    },
    BarColor {
        role: String,
    },
    BarVisible {
        visible: bool,
    },
    BarTooltip {
        value: Value,
    },
    BarImage {
        path: String,
        preserve: bool,
        width: u32,
        height: u32,
    },
    BarRender {
        tree: UiNode,
    },
    PanelRender {
        tree: UiNode,
    },
    DesktopWidgetRender {
        tree: UiNode,
    },
    LayerShow {
        id: String,
    },
    LayerHide {
        id: String,
    },
    LayerRender {
        tree: UiNode,
    },
    LayerConfigure {
        id: String,
        props: Value,
    },
    LauncherResults {
        text: String,
        rows: Vec<Value>,
    },
    RemoveFile {
        path: String,
    },
    HostCall {
        root: String,
        path: Vec<String>,
        args: Vec<Value>,
    },
    Query {
        id: String,
        root: String,
        path: Vec<String>,
        args: Vec<Value>,
        response: String,
    },
}

impl Op {
    #[inline]
    #[must_use]
    pub fn log(message: impl Into<String>) -> Self {
        Self::Log {
            message: message.into(),
        }
    }

    #[inline]
    #[must_use]
    pub const fn log_static(message: &'static str) -> Self {
        Self::LogShared {
            message: SharedStr(message),
        }
    }

    #[inline]
    #[must_use]
    pub fn state(key: &str, value: Value) -> Self {
        Self::StateSet {
            key: key.to_owned(),
            value,
        }
    }

    #[inline]
    #[must_use]
    pub const fn state_static(
        key: &'static str,
        value: &'static Value,
    ) -> Self {
        Self::StateSetShared {
            key: SharedStr(key),
            value: SharedValue(value),
        }
    }

    #[inline]
    #[must_use]
    pub fn host_call(root: &str, path: &[&str], args: Vec<Value>) -> Self {
        Self::HostCall {
            root: root.to_owned(),
            path: path.iter().map(|item| (*item).to_owned()).collect(),
            args,
        }
    }

    #[inline]
    #[expect(
        clippy::too_many_arguments,
        reason = "one op carries the full query envelope"
    )]
    #[must_use]
    pub fn query(
        id: &str,
        root: &str,
        path: &[&str],
        args: Vec<Value>,
        response: String,
    ) -> Self {
        Self::Query {
            id: id.to_owned(),
            root: root.to_owned(),
            path: path.iter().map(|item| (*item).to_owned()).collect(),
            args,
            response,
        }
    }

    #[inline]
    #[must_use]
    pub fn http(request: Value, callback_id: Option<&str>) -> Self {
        let args = match callback_id {
            Some(id) => vec![request, callback(id)],
            None => vec![request],
        };
        Self::host_call("noctalia", &["http"], args)
    }

    #[inline]
    #[must_use]
    pub fn http_bytes(
        request: Value,
        bytes: ByteBuf,
        callback_id: Option<&str>,
    ) -> Self {
        let mut request = request;
        if let Some(object) = request.as_object_mut() {
            object.insert("body".to_owned(), bytes_value(bytes));
        }
        Self::http(request, callback_id)
    }

    #[inline]
    #[must_use]
    pub fn copy_bytes(bytes: ByteBuf, mime_type: &str) -> Self {
        Self::host_call(
            "noctalia",
            &["copyToClipboard"],
            vec![bytes_value(bytes), Value::String(mime_type.to_owned())],
        )
    }

    #[inline]
    #[must_use]
    pub fn write_file_bytes(path: &str, bytes: ByteBuf) -> Self {
        Self::host_call(
            "noctalia",
            &["writeFile"],
            vec![Value::String(path.to_owned()), bytes_value(bytes)],
        )
    }

    #[inline]
    #[must_use]
    pub fn load_font(path: &str) -> Self {
        Self::host_call(
            "noctalia",
            &["loadFont"],
            vec![Value::String(path.to_owned())],
        )
    }

    #[inline]
    #[must_use]
    pub fn run(command: Value, callback_id: Option<&str>) -> Self {
        let args = match callback_id {
            Some(id) => vec![command, callback(id)],
            None => vec![command],
        };
        Self::host_call("noctalia", &["runAsync"], args)
    }

    #[inline]
    #[must_use]
    pub fn run_with_timeout(
        command: Value,
        callback_id: Option<&str>,
        timeout_ms: Option<f64>,
    ) -> Self {
        let mut args = match callback_id {
            Some(id) => vec![command, callback(id)],
            None => vec![command, Value::Null],
        };
        args.push(
            timeout_ms
                .and_then(serde_json::Number::from_f64)
                .map_or(Value::Null, Value::Number),
        );
        Self::host_call("noctalia", &["runAsync"], args)
    }
    #[inline]
    #[must_use]
    pub fn run_stream(command: Value, callback_id: &str) -> Self {
        Self::host_call(
            "noctalia",
            &["runStream"],
            vec![command, callback(callback_id)],
        )
    }

    #[inline]
    #[must_use]
    pub fn state_watch(key: &str, callback_id: &str) -> Self {
        Self::host_call(
            "noctalia",
            &["state", "watch"],
            vec![Value::String(key.to_owned()), callback(callback_id)],
        )
    }

    #[inline]
    #[must_use]
    pub fn notify(title: &str, body: &str) -> Self {
        Self::Notify {
            title: title.to_owned(),
            body: body.to_owned(),
            options: None,
        }
    }

    #[inline]
    #[must_use]
    pub fn notify_with_options(
        title: &str,
        body: &str,
        options: Option<Value>,
    ) -> Self {
        Self::Notify {
            title: title.to_owned(),
            body: body.to_owned(),
            options,
        }
    }

    #[inline]
    #[must_use]
    pub fn notify_error(title: &str, body: &str) -> Self {
        Self::NotifyError {
            title: title.to_owned(),
            body: body.to_owned(),
            options: None,
        }
    }

    #[inline]
    #[must_use]
    pub fn notify_error_with_options(
        title: &str,
        body: &str,
        options: Option<Value>,
    ) -> Self {
        Self::NotifyError {
            title: title.to_owned(),
            body: body.to_owned(),
            options,
        }
    }

    #[inline]
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "one op carries the full timer envelope"
    )]
    pub fn timer_start(
        handle: u64,
        ms: u64,
        callback_id: &str,
        repeat: bool,
    ) -> Self {
        Self::TimerStart {
            handle,
            ms,
            callback: callback(callback_id),
            repeat,
        }
    }

    #[inline]
    #[must_use]
    pub const fn timer_cancel(handle: u64) -> Self {
        Self::TimerCancel { handle }
    }

    #[inline]
    #[must_use]
    pub const fn set_interval(ms: u64) -> Self {
        Self::SetInterval { ms }
    }

    #[inline]
    #[must_use]
    pub fn copy(text: &str) -> Self {
        Self::host_call(
            "noctalia",
            &["copyToClipboard"],
            vec![Value::String(text.to_owned())],
        )
    }

    #[inline]
    #[must_use]
    pub fn copy_with_mime(text: &str, mime_type: &str) -> Self {
        Self::host_call(
            "noctalia",
            &["copyToClipboard"],
            vec![
                Value::String(text.to_owned()),
                Value::String(mime_type.to_owned()),
            ],
        )
    }

    #[inline]
    #[must_use]
    pub fn open_path(path: &str) -> Self {
        Self::host_call(
            "noctalia",
            &["openPath"],
            vec![Value::String(path.to_owned())],
        )
    }

    #[inline]
    #[must_use]
    pub fn osd(icon: &str, value: &str, extra: Option<Value>) -> Self {
        let mut args = vec![
            Value::String(icon.to_owned()),
            Value::String(value.to_owned()),
        ];
        if let Some(options) = extra {
            args.push(options);
        }
        Self::host_call("noctalia", &["osd", "show"], args)
    }

    #[inline]
    #[must_use]
    pub fn set_wallpaper(path: &str) -> Self {
        Self::host_call(
            "noctalia",
            &["setWallpaper"],
            vec![Value::String(path.to_owned())],
        )
    }

    #[inline]
    #[must_use]
    pub fn set_wallpaper_on(connector: &str, path: &str) -> Self {
        Self::host_call(
            "noctalia",
            &["setWallpaper"],
            vec![
                Value::String(connector.to_owned()),
                Value::String(path.to_owned()),
            ],
        )
    }

    #[inline]
    #[must_use]
    pub fn reveal_path(path: &str) -> Self {
        Self::host_call(
            "noctalia",
            &["revealPath"],
            vec![Value::String(path.to_owned())],
        )
    }

    #[inline]
    #[must_use]
    pub fn run_in_terminal(command: &str) -> Self {
        Self::host_call(
            "noctalia",
            &["runInTerminal"],
            vec![Value::String(command.to_owned())],
        )
    }
}

#[macro_export]
macro_rules! state_json {
    ($key:expr, $json:literal) => {{
        static VALUE: ::std::sync::OnceLock<$crate::Value> =
            ::std::sync::OnceLock::new();
        let value: &'static $crate::Value =
            VALUE.get_or_init(|| $crate::ops::parse_json_static($json));
        $crate::Op::state_static($key, value)
    }};
}
#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;
    #[test]
    fn test_log_serializes_with_op_tag() {
        let value = serde_json::to_value(Op::log("hello")).expect("serialize");

        assert_eq!(value, json!({ "op": "log", "message": "hello" }));
    }

    #[test]
    fn test_state_serializes_value() {
        let value = serde_json::to_value(Op::state("key", json!([1, 2])))
            .expect("serialize");

        assert_eq!(
            value,
            json!({ "op": "state_set", "key": "key", "value": [1, 2] })
        );
    }

    #[test]
    fn test_static_ops_match_json_construction() {
        let expected = vec![
            Op::state(
                "bench.state",
                json!({
                    "count": 1,
                    "items": [{
                        "id": "row-1",
                        "tags": ["a", "b"],
                        "value": 42
                    }]
                }),
            ),
            Op::log("published"),
        ];
        let actual = vec![
            crate::state_json!(
                "bench.state",
                r#"{"count":1,"items":[{"id":"row-1","tags":["a","b"],"value":42}]}"#
            ),
            Op::log_static("published"),
        ];

        assert_eq!(
            serde_json::to_string(&expected).expect("serialize expected"),
            serde_json::to_string(&actual).expect("serialize actual")
        );
        assert_eq!(
            serde_json::to_value(&expected).expect("value expected"),
            serde_json::to_value(&actual).expect("value actual")
        );
    }

    #[test]
    fn test_host_call_serializes_path_and_args() {
        let value = serde_json::to_value(Op::host_call(
            "noctalia",
            &["state", "get"],
            vec![json!("key")],
        ))
        .expect("serialize");

        assert_eq!(
            value,
            json!({
                "op": "host_call",
                "root": "noctalia",
                "path": ["state", "get"],
                "args": ["key"]
            })
        );
    }

    #[test]
    fn test_http_embeds_callback_marker() {
        let value =
            serde_json::to_value(Op::http(json!({ "url": "x" }), Some("done")))
                .expect("serialize");

        assert_eq!(value["args"][1], json!({ "__host_callback": "done" }));
    }

    #[test]
    fn test_copy_builds_clipboard_call() {
        let value = serde_json::to_value(Op::copy("text")).expect("serialize");

        assert_eq!(value["path"], json!(["copyToClipboard"]));
        assert_eq!(value["args"], json!(["text"]));
    }

    #[test]
    fn test_run_uses_host_run_async() {
        let value = serde_json::to_value(Op::run(json!(["echo"]), None))
            .expect("serialize");

        assert_eq!(value["path"], json!(["runAsync"]));
        assert_eq!(value["args"], json!([["echo"]]));
    }

    #[test]
    fn test_open_path_and_wallpaper_use_expected_paths() {
        let open =
            serde_json::to_value(Op::open_path("/tmp")).expect("serialize");
        let wallpaper = serde_json::to_value(Op::set_wallpaper("/tmp/a.png"))
            .expect("serialize");

        assert_eq!(open["path"], json!(["openPath"]));
        assert_eq!(wallpaper["path"], json!(["setWallpaper"]));
    }

    #[test]
    fn test_osd_includes_optional_options() {
        let value = serde_json::to_value(Op::osd(
            "volume",
            "50",
            Some(json!({ "progress": 0.5 })),
        ))
        .expect("serialize");

        assert_eq!(value["path"], json!(["osd", "show"]));
        assert_eq!(value["args"][2]["progress"], 0.5);
    }

    #[test]
    fn test_http_without_callback_has_one_argument() {
        let value = serde_json::to_value(Op::http(json!({ "url": "x" }), None))
            .expect("serialize");

        assert_eq!(value["args"].as_array().expect("args").len(), 1);
    }
    #[test]
    fn test_stream_and_watch_ops_use_host_callbacks() {
        let stream = serde_json::to_value(Op::run_stream(
            json!("niri msg --json event-stream"),
            "line",
        ))
        .expect("serialize");
        let watch =
            serde_json::to_value(Op::state_watch("wm.workspaces", "changed"))
                .expect("serialize");

        assert_eq!(stream["path"], json!(["runStream"]));
        assert_eq!(stream["args"][1], json!({ "__host_callback": "line" }));
        assert_eq!(watch["path"], json!(["state", "watch"]));
        assert_eq!(watch["args"][1], json!({ "__host_callback": "changed" }));
    }

    #[test]
    fn test_notify_and_interval_ops() {
        let notice = serde_json::to_value(Op::notify("Done", "Body"))
            .expect("serialize");
        let error = serde_json::to_value(Op::notify_error("Failed", "Body"))
            .expect("serialize");
        let interval =
            serde_json::to_value(Op::set_interval(1000)).expect("serialize");

        assert_eq!(notice["op"], json!("notify"));
        assert_eq!(error["op"], json!("notify_error"));
        assert_eq!(interval, json!({ "op": "set_interval", "ms": 1000 }));
    }
}
