//! Local Luau timer helpers emitted into a compiled entry.
//!
//! Plugin API 57 has no one-shot timer member. Both modes build timers from
//! `setUpdateInterval` and `nowMs`: a pending timer shortens the update
//! interval while it is active, and an explicit `Op::set_interval` records the
//! base interval that is restored when no timer is pending.

pub(crate) fn source(strict: bool) -> String {
    let dispatch = if strict {
        "    item[2]()\n"
    } else {
        "    local dispatch = onHostCallback\n\
         \x20   if type(dispatch) == \"function\" then\n\
         \x20     dispatch({ id = item[2], args = {} })\n\
         \x20   end\n"
    };
    let expected = if strict { "function" } else { "string" };
    format!(
        concat!(
            "local __noctalia_timer_base_ms = 1000\n",
            "local __noctalia_timer_tick_ms = 1000\n",
            "local __noctalia_timer_next = 0\n",
            "local __noctalia_timer_last_now = nil\n",
            "local __noctalia_timers = {{}}\n",
            "local function __noctalia_timer_apply(target)\n",
            "  if target < 16 then target = 16 end\n",
            "  if target ~= __noctalia_timer_tick_ms then\n",
            "    __noctalia_timer_tick_ms = target\n",
            "    noctalia.setUpdateInterval(target)\n",
            "  end\n",
            "end\n",
            "local function __noctalia_timer_schedule()\n",
            "  local now = noctalia.nowMs() or 0\n",
            "  local next_ms = nil\n",
            "  for _, timer in pairs(__noctalia_timers) do\n",
            "    local remaining = timer.at - now\n",
            "    if remaining < 0 then remaining = 0 end\n",
            "    if next_ms == nil or remaining < next_ms then next_ms = remaining end\n",
            "  end\n",
            "  local target = __noctalia_timer_base_ms\n",
            "  if next_ms ~= nil and next_ms < target then target = next_ms end\n",
            "  __noctalia_timer_apply(target)\n",
            "end\n",
            "local function __noctalia_timer_tick(force)\n",
            "  local now = noctalia.nowMs() or 0\n",
            "  if not force and __noctalia_timer_last_now == now then return end\n",
            "  __noctalia_timer_last_now = now\n",
            "  local due = {{}}\n",
            "  for handle, timer in pairs(__noctalia_timers) do\n",
            "    if now >= timer.at then\n",
            "      due[#due + 1] = {{ handle, timer.callback }}\n",
            "      if timer.every and timer.every > 0 then\n",
            "        local missed = math.floor((now - timer.at) / timer.every) + 1\n",
            "        timer.at = timer.at + missed * timer.every\n",
            "      else\n",
            "        __noctalia_timers[handle] = nil\n",
            "      end\n",
            "    end\n",
            "  end\n",
            "  table.sort(due, function(left, right) return left[1] < right[1] end)\n",
            "  for _, item in ipairs(due) do\n",
            "{dispatch}",
            "  end\n",
            "  __noctalia_timer_schedule()\n",
            "end\n",
            "local function __noctalia_timer_base(ms)\n",
            "  __noctalia_timer_base_ms = ms\n",
            "  __noctalia_timer_schedule()\n",
            "end\n",
            "local function __noctalia_timer_start(ms, callback, every, at)\n",
            "  if type(ms) ~= \"number\" or ms % 1 ~= 0 or ms < 0 or ms > 9223372036854775807 then\n",
            "    error(at .. \": timer delay is outside the exact Luau integer range\")\n",
            "  end\n",
            "  if type(callback) ~= \"{expected}\" or (every and ms == 0) then\n",
            "    error(at .. \": invalid timer callback or interval period\")\n",
            "  end\n",
            "  __noctalia_timer_next += 1\n",
            "  __noctalia_timers[__noctalia_timer_next] = {{\n",
            "    at = noctalia.nowMs() + ms,\n",
            "    every = every and ms or nil,\n",
            "    callback = callback,\n",
            "  }}\n",
            "  __noctalia_timer_schedule()\n",
            "  return __noctalia_timer_next\n",
            "end\n",
            "local function __noctalia_timer_cancel(handle)\n",
            "  __noctalia_timers[handle] = nil\n",
            "  __noctalia_timer_schedule()\n",
            "end\n"
        ),
        dispatch = dispatch,
        expected = expected,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_helpers_use_only_plugin_api_calls() {
        let source = source(true);
        assert!(source.contains("noctalia.setUpdateInterval(target)"));
        assert!(source.contains("noctalia.nowMs()"));
        assert!(!source.contains("onHostCallback"));
        assert!(!source.contains("__host_callback"));
    }

    #[test]
    fn auto_helpers_dispatch_by_callback_id() {
        let source = source(false);
        assert!(source.contains("dispatch({ id = item[2], args = {} })"));
    }
}
