//! Strict handle helpers emitted into the one generated entry script.
//!
//! API 57 gives a real stop function only to `httpStream`. Run, HTTP and state
//! watch handles therefore suppress their callback instead of claiming to kill
//! host work the frozen API cannot cancel.

pub(super) const RUN: &str = "RunHandle";
pub(super) const HTTP: &str = "HttpHandle";
pub(super) const STREAM: &str = "HttpStreamHandle";
pub(super) const WATCH: &str = "StateWatchHandle";

pub(super) fn is_class(name: &str) -> bool {
    matches!(name, RUN | HTTP | STREAM | WATCH)
}
pub(crate) fn runtime(
    run: bool,
    http: bool,
    stream: bool,
    watch: bool,
) -> String {
    let mut out = String::new();
    if run {
        out.push_str(concat!(
            "local function __strict_run_handle(__request, __callback, __timeout)\n",
            "  local active = true\n",
            "  local started\n",
            "  if __callback then\n",
            "    local callback = (function(__captured) return function(...) if active then __strict_callback(__captured, ...) end end end)(__callback)\n",
            "    started = noctalia.runAsync(__request, callback, __timeout)\n",
            "  else\n",
            "    started = noctalia.runAsync(__request, nil, __timeout)\n",
            "  end\n",
            "  return { stop = function() active = false end, started = function() return started end }\n",
            "end\n",
        ));
    }
    if http {
        out.push_str(concat!(
            "local function __strict_http_handle(__request, __callback)\n",
            "  local active = true\n",
            "  local callback = (function(__captured) return function(...) if active then __strict_callback(__captured, ...) end end end)(__callback)\n",
            "  local started = noctalia.http(__request, callback)\n",
            "  return { stop = function() active = false end, started = function() return started end }\n",
            "end\n",
        ));
    }
    if stream {
        out.push_str(concat!(
            "local function __strict_http_stream_handle(__request, __line, __close)\n",
            "  local active = true\n",
            "  local line = (function(__captured) return function(...) if active then __strict_callback(__captured, ...) end end end)(__line)\n",
            "  local close = (function(__captured) return function(...) if active then __strict_callback(__captured, ...) end end end)(__close)\n",
            "  local handle = noctalia.httpStream(__request, line, close)\n",
            "  return {\n",
            "    stop = function()\n",
            "      active = false\n",
            "      if handle then handle.stop() end\n",
            "    end,\n",
            "    started = function() return handle ~= nil end,\n",
            "  }\n",
            "end\n",
        ));
    }
    if watch {
        out.push_str(concat!(
            "local function __strict_state_watch_handle(__key, __callback)\n",
            "  local active = true\n",
            "  state.watch(__key, (function(__captured) return function(...) if active then __strict_callback(__captured, ...) end end end)(__callback))\n",
            "  return { stop = function() active = false end, started = function() return __key ~= \"\" and __callback ~= \"\" end }\n",
            "end\n",
        ));
    }
    out
}
