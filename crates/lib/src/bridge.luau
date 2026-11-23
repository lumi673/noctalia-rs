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
  "secondary",
  "tertiary",
  "error",
  "on_surface",
  "on_surface_variant",
  "outline_variant",
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
local sequence = 0
local pending = {}
local stream_started = false

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
  local host = {
    now_ms = invoke(noctalia, "nowMs") or 0,
    dark = invoke(noctalia, "isDarkMode"),
    outputs = invoke(noctalia, "outputs"),
    clipboard = invoke(noctalia, "clipboardText"),
    calendar = invoke(noctalia, "calendarEvents"),
    media = invoke(noctalia, "mediaPlayer"),
    stats = invoke(noctalia, "systemStats"),
    nightlight = invoke(noctalia, "nightlight.state"),
    compositor = invoke(noctalia, "compositor.name"),
    bar = {
      vertical = invoke(barWidget, "isVertical"),
      output_name = invoke(barWidget, "outputName"),
    },
    colors = {},
  }
  for _, role in ipairs(COLOR_ROLES) do
    host.colors[role] = invoke(noctalia, "getColor", role)
  end
  return host
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

local handlers = {
  log = function(op) noctalia.log(op.message) end,
  notify = function(op) noctalia.notify(op.title, op.body) end,
  notify_error = function(op) noctalia.notifyError(op.title, op.body) end,
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
  layer_render = function(op) noctalia.layer.render(render_node(op.tree)) end,
  layer_configure = function(op) noctalia.layer.configure(op.id, op.props) end,
  remove_file = function(op) noctalia.removeFile(op.path) end,
  host_call = function(op)
    local target = resolve(op.root, op.path)
    if type(target) ~= "function" then
      noctalia.log("unknown host call: " .. tostring(op.root))
      return
    end
    local args = {}
    for index, value in ipairs(op.args) do
      args[index] = materialize(value)
    end
    local ok, err = pcall(target, table.unpack(args))
    if not ok then
      noctalia.log(tostring(err))
    end
  end,
}

local function shellQuote(value)
  return "'" .. tostring(value):gsub("'", "'\\''") .. "'"
end

local function writeRequest(request)
  sequence += 1
  local name = string.format("%012d", sequence)
  local temporary = REQUEST_DIR .. "/" .. name .. ".pending"
  local target = REQUEST_DIR .. "/" .. name .. ".json"
  M.host = snapshot()
  request.host = M.host
  local ok, err = noctalia.writeFile(temporary, noctalia.json.encode(request))
  if ok then
    ok, err = noctalia.renameFile(temporary, target)
  end
  if not ok then
    noctalia.removeFile(temporary)
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
    boot = message.boot
    REQUEST_DIR = message.spool or SPOOL_DIR
    flushPending()
    return
  end
  if message.control == "error" then
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

function M.rerender_query()
  if M.last_query ~= nil then
    M.call("query", { text = M.last_query })
  end
end

return M
