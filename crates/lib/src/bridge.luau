--!nonstrict

local M = {}

local BIN = noctalia.pluginDir() .. "/{{HELPER}}"
local DATA_DIR = noctalia.pluginDataDir() or noctalia.pluginDir()
local RUNTIME_ID = (tostring({}):gsub("[^%w_%-]", "_"))
local SPOOL_DIR = DATA_DIR .. "/.noctalia-rs/spool"
local REQUEST_DIR = nil
local STATE_KEY = "{{PREFIX}}.state"


local ROOTS = {
  noctalia = noctalia,
  barWidget = barWidget,
  desktopWidget = desktopWidget,
  shortcut = shortcut,
  panel = panel,
  launcher = launcher,
  layer = layer,
  ui = ui,
}

local COLOR_ROLES = {
  "primary",
  "on_primary",
  "primary_container",
  "on_primary_container",
  "secondary",
  "on_secondary",
  "secondary_container",
  "on_secondary_container",
  "tertiary",
  "on_tertiary",
  "tertiary_container",
  "on_tertiary_container",
  "error",
  "on_error",
  "error_container",
  "on_error_container",
  "surface",
  "on_surface",
  "on_surface_variant",
  "surface_container_low",
  "surface_container",
  "surface_container_high",
  "surface_container_highest",
  "outline_variant",
  "shadow",
  "scrim",
  "hover",
  "on_hover",
  "surface_variant",
  "outline",
}

local SETTING_PATHS = {
  "shell.time_format",
  "shell.date_format",
  "shell.offline_mode",
  "shell.lang",
  "shell.language",
  "shell.corner_radius_scale",
  "shell.show_close_buttons",
  "general.language",
  "bar",
  "bar.order",
  "widget",
  "desktop_widgets",
  "desktop_widgets.widget",
  "theme.source",
  "theme.pure_black_dark",
  "theme.wallpaper_scheme",
  "system.monitor.network_poll_seconds",
}

M.entry = "unknown"
M.config = {}
M.state = {}
M.host = {}
local function safeName(value)
  return (tostring(value):gsub("[^%w_%.%-]", "_"))
end
M.last_query = nil

local boot = nil
local fixed_host = nil
local fixed_sent = false
local sequence = 0
local pending = {}
local stream_started = false
local handles = {}
local function hexDecode(text)
  return (string.gsub(text or "", "%x%x", function(pair)
    return string.char(tonumber(pair, 16) or 0)
  end))
end

local function hexEncode(value)
  return (string.gsub(value, ".", function(byte)
    return string.format("%02x", string.byte(byte))
  end))
end

local function isBinary(value)
  if type(value) ~= "string" then return false end
  local ok, length = pcall(utf8.len, value)
  return not ok or length == nil
end

local function encodeBytes(value)
  if isBinary(value) then
    return { __noctalia_rs_bytes = hexEncode(value) }
  end
  if type(value) ~= "table" then return value end
  local out = {}
  for key, item in pairs(value) do
    out[key] = encodeBytes(item)
  end
  return out
end


local function materialize(value)
  if type(value) ~= "table" then
    return value
  end
  if type(value.__call) == "string" then
    local name = value.__call
    return function(...)
      M.call("action", { name = name, args = { ... } })
    end
  end
  if type(value.__host_callback) == "string" then
    local name = value.__host_callback
    return function(...)
      M.call("host_callback", { id = name, args = { ... } })
    end
  end
  local marker = value.__noctalia_rs_bytes
  if type(marker) == "string" then
    local only = next(value)
    if only == "__noctalia_rs_bytes" and next(value, only) == nil
      and #marker % 2 == 0 and marker:match("^%x*$") ~= nil then
      return hexDecode(marker)
    end
  end
  local out = {}
  for key, item in pairs(value) do
    out[key] = materialize(item)
  end
  return out
end

local function render_node(node)
  local ctor = ui[node.kind]
  if not ctor then
    return ui.label({ text = "unknown ui node: " .. tostring(node.kind) })
  end
  local props = {}
  for key, item in pairs(node.props or {}) do
    props[key] = materialize(item)
  end
  local children = {}
  for index, child in ipairs(node.children or {}) do
    children[index] = render_node(child)
  end
  if #children == 0 then
    return ctor(props)
  end
  return ctor(props, children)
end

local function invoke(root, path, ...)
  local value = root
  for part in string.gmatch(path, "[^.]+") do
    if type(value) ~= "table" then
      return nil
    end
    value = value[part]
  end
  if type(value) ~= "function" then
    return nil
  end
  local ok, result = pcall(value, ...)
  if ok then
    return result
  end
  return nil
end
local function snapshot()
  local fixed = {
    pluginDir = invoke(noctalia, "pluginDir"),
    pluginDataDir = invoke(noctalia, "pluginDataDir"),
    compositor = { name = invoke(noctalia, "compositor.name") },
    setting = {},
    bar = {
      vertical = invoke(barWidget, "isVertical"),
      output_name = invoke(barWidget, "outputName"),
    },
    colors = {},
  }
  for _, path in ipairs(SETTING_PATHS) do
    fixed.setting[path] = invoke(noctalia, "getSetting", path)
  end
  for _, role in ipairs(COLOR_ROLES) do
    fixed.colors[role] = invoke(noctalia, "getColor", role)
  end
  return {
    now_ms = invoke(noctalia, "nowMs") or 0,
    dark = invoke(noctalia, "isDarkMode"),
    outputs = invoke(noctalia, "outputs"),
    focused_output_name = invoke(noctalia, "focusedOutputName"),
    clipboard = invoke(noctalia, "clipboardText"),
    calendar = invoke(noctalia, "calendarEvents"),
    media = invoke(noctalia, "mediaPlayer"),
    stats = invoke(noctalia, "systemStats"),
    nightlight = invoke(noctalia, "nightlight.state"),
  }, fixed
end

local function sameValue(left, right)
  if left == right then return true end
  if type(left) ~= "table" or type(right) ~= "table" then return false end
  for key, value in pairs(left) do
    if not sameValue(value, right[key]) then return false end
  end
  for key in pairs(right) do
    if left[key] == nil then return false end
  end
  return true
end

local function resolve(root, path)
  local value = ROOTS[root]
  if not value then
    return nil
  end
  for _, part in ipairs(path) do
    if type(value) ~= "table" then
      return nil
    end
    value = value[part]
  end
  return value
end
local function call_host(root, path, args)
  local target = resolve(root, path)
  if type(target) ~= "function" then
    return false, "unknown host call: " .. tostring(root) .. "." .. table.concat(path or {}, ".")
  end
  local values = {}
  for index, value in ipairs(args or {}) do
    values[index] = materialize(value)
  end
  return pcall(target, table.unpack(values))
end


local handlers = {
  log = function(op) noctalia.log(op.message) end,
  notify = function(op) noctalia.notify(op.title, op.body, op.options) end,
  notify_error = function(op) noctalia.notifyError(op.title, op.body, op.options) end,
  set_interval = function(op) noctalia.setUpdateInterval(op.ms) end,
  state_set = function(op) noctalia.state.set(op.key, op.value) end,
  toggle_panel = function(op) noctalia.togglePanel(op.id) end,
  open_settings = function(op) noctalia.openSettings() end,
  bar_text = function(op) barWidget.setText(op.text) end,
  bar_color = function(op) barWidget.setColor(op.role) end,
  bar_visible = function(op) barWidget.setVisible(op.visible) end,
  bar_tooltip = function(op) barWidget.setTooltip(op.value) end,
  bar_image = function(op)
    barWidget.setImage(op.path, op.preserve, op.width, op.height)
  end,
  bar_render = function(op) barWidget.render(render_node(op.tree)) end,
  panel_render = function(op) panel.render(render_node(op.tree)) end,
  desktop_widget_render = function(op)
    desktopWidget.render(render_node(op.tree))
  end,
  launcher_results = function(op) launcher.setResults(op.text, op.rows) end,
  layer_show = function(op) noctalia.layer.show(op.id) end,
  layer_hide = function(op) noctalia.layer.hide(op.id) end,
  layer_render = function(op) layer.render(render_node(op.tree)) end,
  layer_configure = function(op) noctalia.layer.configure(op.id, op.props) end,
  remove_file = function(op) noctalia.removeFile(op.path) end,
  host_call = function(op)
    if op.root == "__noctalia_rs" and op.path[1] == "httpStreamStop" then
      local handle = handles[op.args[1]]
      if handle then
        handles[op.args[1]] = nil
        handle.stop()
      end
      return
    end
    local ok, err = call_host(op.root, op.path, op.args)
    if not ok then
      noctalia.log(tostring(err))
    end
  end,
  query = function(op)
    local ok, value, err = call_host(op.root, op.path, op.args)
    local response = { ok = ok, value = value, error = err }
    if op.root == "noctalia" and op.path[1] == "httpStream" and ok then
      handles[op.id] = value
      response.value = op.id
      response.error = nil
    end
    if not ok then
      response.value = nil
      response.error = tostring(value)
    end
    local encoded = noctalia.json.encode(encodeBytes(response))
    if encoded then
      local temporary = op.response .. ".pending"
      local written, write_err = noctalia.writeFile(temporary, encoded)
      if written then
        written, write_err = noctalia.renameFile(temporary, op.response)
      end
      if not written then
        noctalia.removeFile(temporary)
        noctalia.log("noctalia bridge: query response failed: " .. tostring(write_err))
      end
    end
  end,
}

local function shellQuote(value)
  return "'" .. tostring(value):gsub("'", "'\\''") .. "'"
end

local function writeRequest(request)
  sequence += 1
  local target = REQUEST_DIR .. "/" .. string.format("%012d", sequence) .. ".json"
  local host, fixed = snapshot()
  M.host = {}
  for key, value in pairs(fixed) do M.host[key] = value end
  for key, value in pairs(host) do M.host[key] = value end
  request.host = host
  if not fixed_sent or not sameValue(fixed, fixed_host) then
    fixed_host = fixed
    fixed_sent = true
    request.host_fixed = fixed
  end
  -- Written in place. The helper wakes on the create and retries a truncated
  -- payload, so the round trip overlaps the write instead of waiting for a
  -- rename to publish the file.
  local ok, err = noctalia.writeFile(target, noctalia.json.encode(encodeBytes(request)))
  if not ok then
    noctalia.removeFile(target)
    noctalia.log("noctalia bridge: spool write failed: " .. tostring(err))
  end
end

local function flushPending()
  if not boot then
    return
  end
  local queued = pending
  pending = {}
  for _, request in ipairs(queued) do
    request.boot = boot
    writeRequest(request)
  end
end

local function onLine(line)
  local ok, message = pcall(noctalia.json.decode, line)
  if not ok or type(message) ~= "table" then
    return
  end
  if message.control == "ready" then
    -- A fresh helper has no cached fixed snapshot.
    fixed_sent = false
    boot = message.boot
    REQUEST_DIR = message.spool or SPOOL_DIR
    flushPending()
    return
  end
  if message.control == "error" then
    -- The helper may have rejected the request that carried the fixed part,
    -- so it has to travel again with the next one.
    fixed_sent = false
    noctalia.log("noctalia bridge: " .. tostring(message.message))
    return
  end
  if message.control ~= nil then
    return
  end
  if type(message.op) ~= "string" then
    return
  end
  local handler = handlers[message.op]
  if handler then
    handler(message)
  end
end

local function startStream()
  if stream_started then
    return
  end

  if noctalia.mkdirAll then
    noctalia.mkdirAll(SPOOL_DIR)
  end
  local command = shellQuote(BIN)
    .. " serve "
    .. shellQuote(DATA_DIR)
    .. " "
    .. shellQuote(SPOOL_DIR)
  local loop = "while kill -0 \"$PPID\" 2>/dev/null; do "
    .. command
    .. "; sleep 0.2; done"
  if noctalia.runStream(loop, onLine) then
    stream_started = true
  else
    noctalia.log("noctalia bridge: could not start persistent helper")
  end
end

function M.call(event, payload)
  if not boot then startStream() end
  table.insert(pending, {
    entry = M.entry,
    event = event,
    payload = payload or {},
    config = M.config,
    state = M.state,
  })
  flushPending()
end

function M.setup(entry, configKeys)
  M.entry = entry
  SPOOL_DIR = DATA_DIR .. "/.noctalia-rs/spool/" .. safeName(entry) .. "-" .. RUNTIME_ID
  M.config = {}
  for _, key in ipairs(configKeys) do
    M.config[key] = noctalia.getConfig(key)
  end
  M.state = noctalia.state.get(STATE_KEY) or {}
  M.host = snapshot()
  startStream()
end

function M.watch_state(rerender)
  noctalia.state.watch(STATE_KEY, function(value)
    M.state = value or {}
    rerender()
  end)
end

function M.query(text)
  M.last_query = text
  M.call("query", { text = text })
end

function M.rerender_query(source)
  if M.last_query ~= nil then
    M.call("query", { text = M.last_query, source = source or "state" })
  end
end

return M
