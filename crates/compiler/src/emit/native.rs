use super::*;

impl<'a> Emitter<'a> {
    /// Records `helper` as a native function and validates that its signature
    /// can cross the JSON boundary.
    pub(super) fn register_native(
        &mut self,
        helper: &Helper,
        cache_ms: u64,
        forced: bool,
        reason: &str,
        _span: proc_macro2::Span,
    ) -> Result<NativeFn, Error> {
        if is_luau_attr(&helper.function) {
            let mut error = Error::fatal(
                helper.function.sig.ident.span(),
                format!(
                    "`{}` is marked `#[luau]` but cannot be compiled to Luau: {reason}",
                    helper.function.sig.ident
                ),
            );
            if helper.module != self.home {
                error = error.with_file(&helper.module);
            }
            return Err(error);
        }
        let params = match native_signature(&helper.function) {
            Ok(params) => params,
            Err(mut error) => {
                error.fatal = forced;
                if helper.module != self.home {
                    error = error.with_file(&helper.module);
                }
                return Err(error);
            }
        };
        let exported = matches!(
            helper.function.vis,
            syn::Visibility::Public(_) | syn::Visibility::Restricted(_)
        );
        if !exported && helper.module != self.program.root() {
            let mut error = Error::at(
                helper.function.sig.ident.span(),
                format!(
                    "native function `{}` must be `pub(crate)` so the generated dispatch can call it",
                    helper.function.sig.ident
                ),
            );
            error.fatal = forced;
            if helper.module != self.home {
                error = error.with_file(&helper.module);
            }
            return Err(error);
        }
        if !forced && let Err(mut error) = self.json_ready(helper) {
            if helper.module != self.home {
                error = error.with_file(&helper.module);
            }
            return Err(error);
        }
        let item = NativeFn {
            name: native_key(helper),
            module: helper.module.clone(),
            function: helper.function.sig.ident.to_string(),
            class: helper.class.clone(),
            result: matches!(
                &helper.function.sig.output,
                syn::ReturnType::Type(_, ty) if is_result(ty)
            ),
            params,
            cache_ms,
            reason: reason.to_owned(),
        };
        self.native_found.insert(item.name.clone(), item.clone());
        Ok(item)
    }

    /// Auto-splitting only crosses types the build can prove are JSON-ready.
    /// `#[native]` skips this and leaves the trait check to `rustc`.
    fn json_ready(&self, helper: &Helper) -> Result<(), Error> {
        for (pattern, ty) in typed_args(&helper.function) {
            if is_context(ty) {
                continue;
            }
            let ready = match ty {
                Type::Reference(reference) => json_ready_type(
                    self.program,
                    &helper.module,
                    &reference.elem,
                    Direction::Deserialize,
                ),
                Type::Path(path)
                    if path
                        .path
                        .segments
                        .last()
                        .is_some_and(|segment| segment.ident == "Option") =>
                {
                    match generic_arg(ty) {
                        Some(Type::Reference(reference)) => json_ready_type(
                            self.program,
                            &helper.module,
                            &reference.elem,
                            Direction::Deserialize,
                        ),
                        _ => json_ready_type(
                            self.program,
                            &helper.module,
                            ty,
                            Direction::Deserialize,
                        ),
                    }
                }
                _ => json_ready_type(
                    self.program,
                    &helper.module,
                    ty,
                    Direction::Deserialize,
                ),
            };
            if !ready {
                return Err(Error::at(
                    pattern.span(),
                    format!(
                        "native function `{}` has a parameter type `{}` that cannot be proven to deserialize from JSON; derive `Deserialize` or mark the function `#[native]`",
                        helper.function.sig.ident,
                        type_text(ty)
                    ),
                ));
            }
        }
        if let syn::ReturnType::Type(_, ty) = &helper.function.sig.output {
            let success = if is_result(ty) {
                generic_arg(ty).unwrap_or(ty)
            } else {
                ty
            };
            if !json_ready_type(
                self.program,
                &helper.module,
                success,
                Direction::Serialize,
            ) {
                return Err(Error::at(
                    ty.span(),
                    format!(
                        "native function `{}` has a return type `{}` that cannot be proven to serialize to JSON; derive `Serialize` or mark the function `#[native]`",
                        helper.function.sig.ident,
                        type_text(success)
                    ),
                ));
            }
        }
        Ok(())
    }

    pub(super) fn native_call(
        &mut self,
        item: &NativeFn,
        value_ty: Ty,
        mode: Inline,
        span: proc_macro2::Span,
        bindings: &[Binding],
    ) -> Result<Inlined, Error> {
        if self.deferred > 0 {
            return Err(Error::at(
                span,
                format!(
                    "native function `{}` cannot run inside a closure or callback; call it from the handler body or a helper",
                    item.name
                ),
            ));
        }
        if mode == Inline::Ops {
            return Err(Error::at(
                span,
                format!(
                    "native function `{}` does not build a `Vec<Op>`",
                    item.name
                ),
            ));
        }
        let mut values = Vec::new();
        for binding in bindings {
            match binding {
                Binding::Context(_) => self.uses_native_context = true,
                Binding::Local { text, .. } => values.push(text.clone()),
                Binding::Ops(_) => {
                    return Err(Error::at(
                        span,
                        format!(
                            "native function `{}` cannot take a `Vec<Op>` parameter",
                            item.name
                        ),
                    ));
                }
            }
        }
        let call =
            format!("{}({})", native_local(&item.name), values.join(", "));
        self.uses_native = true;
        let ty = if item.result {
            Ty::result(value_ty.base)
        } else {
            Ty {
                result: false,
                ..value_ty
            }
        };
        match mode {
            Inline::Value => Ok(Some((call, ty))),
            Inline::Statement => {
                self.line(&call);
                Ok(Some((String::new(), Ty::VOID)))
            }
            Inline::Tail => {
                if item.result {
                    // The stub returns `value, error`, so a tail call must not
                    // drop the error on the way to the parent's value.
                    let value = self.temp();
                    let error = self.temp();
                    self.line(&format!("local {value}, {error} = {call}"));
                    self.line(&format!(
                        "if {error} ~= nil then error(tostring({error})) end"
                    ));
                    if let Some(target) = self.tail_target.clone() {
                        self.line(&format!("{target} = {value}"));
                    } else {
                        self.line(&format!("return {value}"));
                    }
                } else if let Some(target) = self.tail_target.clone() {
                    self.line(&format!("{target} = {call}"));
                } else {
                    self.line(&format!("return {call}"));
                }
                Ok(Some((String::new(), Ty::VOID)))
            }
            Inline::Ops => unreachable!("ops mode is rejected above"),
        }
    }
}

/// Dispatch key for a native function. Unique per crate.
pub(super) fn native_key(helper: &Helper) -> String {
    let name = helper.function.sig.ident.to_string();
    match &helper.class {
        Some(class) => format!("{}::{class}::{name}", helper.module),
        None => format!("{}::{name}", helper.module),
    }
}

/// The Luau local that names a native function's stub.
pub(crate) fn native_local(name: &str) -> String {
    let mut out = String::from("__native_fn_");
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            out.push(character);
        } else if character == '_' {
            out.push_str("__");
        } else {
            out.push('_');
        }
    }
    out
}

/// Renders the Luau stub that calls a native function through `__native`.
pub(crate) fn stub_source(item: &NativeFn) -> String {
    let mut declared = Vec::new();
    let mut fields = Vec::new();
    let mut index = 0usize;
    let mut needs_context = false;
    for param in &item.params {
        if param.kind == NativeParamKind::Context {
            needs_context = true;
            continue;
        }
        index = index.saturating_add(1);
        let name = &param.name;
        let local = if is_luau_keyword(name) {
            format!("{name}_{index}")
        } else {
            name.clone()
        };
        declared.push(local.clone());
        let key = if is_identifier(name) && !is_luau_keyword(name) {
            name.clone()
        } else {
            format!("[{}]", quote(name))
        };
        fields.push(format!("{key} = {local}"));
    }
    let args = if fields.is_empty() {
        "{}".to_owned()
    } else {
        format!("{{ {} }}", fields.join(", "))
    };
    let mut out = String::new();
    let _ = write!(
        out,
        "local function {}({})\n  return __native.call({}, {args}, {}, {needs_context}, {})\nend\n\n",
        native_local(&item.name),
        declared.join(", "),
        quote(&item.name),
        item.cache_ms,
        item.result
    );
    out
}

/// The Luau side of the native boundary: request writing, caching, the
/// invocation context, and the coroutine that parks while the helper computes.
pub(crate) fn native_runtime(
    entry: &str,
    state_key: &str,
    settings: &[&str],
) -> String {
    // Kept in step with `crates/lib/src/bridge.luau`: a native function that
    // takes `&Context` must see the same host snapshot the bridge sends.
    pub(crate) const SETTING_PATHS: &[&str] = &[
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
    ];
    pub(crate) const COLOR_ROLES: &[&str] = &[
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
    ];
    let mut out = String::new();
    out.push_str("local __native = (function()\n");
    let _ = writeln!(out, "  local ENTRY = {}", quote(entry));
    let _ = writeln!(out, "  local STATE_KEY = {}", quote(state_key));
    out.push_str(
        r#"  local BIN = noctalia.pluginDir() .. "/{{HELPER}}"
  local DATA_DIR = noctalia.pluginDataDir() or noctalia.pluginDir()
  local RUNTIME_ID = (tostring({}):gsub("[^%w_%-]", "_"))
  local SPOOL_DIR = DATA_DIR .. "/.noctalia-rs/native/" .. ENTRY:gsub("[^%w_]", "_") .. "-" .. RUNTIME_ID
  local CACHE_LIMIT = 128
"#,
    );
    let _ = writeln!(out, "  local CONFIG_KEYS = {}", lua_list(settings));
    let _ =
        writeln!(out, "  local SETTING_PATHS = {}", lua_list(SETTING_PATHS));
    let _ = writeln!(out, "  local COLOR_ROLES = {}", lua_list(COLOR_ROLES));
    out.push_str(
        r#"  local M = {}
  local queue, active, current = {}, nil, nil
  local pending, boot, request_dir, sequence = {}, nil, nil, 0
  local cache, order = {}, {}

  local function shellQuote(value)
    return "'" .. tostring(value):gsub("'", "'\\''") .. "'"
  end

  local function write(request)
    sequence += 1
    local name = string.format("%012d", sequence)
    local temporary = request_dir .. "/" .. name .. ".pending"
    local target = request_dir .. "/" .. name .. ".json"
    request.boot = boot
    request.entry = ENTRY
    request.id = name
    local ok, err = noctalia.writeFile(temporary, noctalia.json.encode(request))
    if ok then ok, err = noctalia.renameFile(temporary, target) end
    if not ok then
      noctalia.removeFile(temporary)
      noctalia.log("noctalia native: spool write failed for " .. name .. ": " .. tostring(err))
      return false
    end
    return true
  end

  function M.send(request)
    if boot and write(request) then return true end
    -- Keep the request for the next ready line: a helper restart retries it.
    pending[#pending + 1] = request
    return true
  end

  local function remember(key, value)
    if cache[key] == nil then order[#order + 1] = key end
    cache[key] = { at = noctalia.nowMs(), value = value }
    while #order > CACHE_LIMIT do
      cache[table.remove(order, 1)] = nil
    end
  end

  local function resolve(root, path)
    local value = root
    for part in string.gmatch(path, "[^.]+") do
      if type(value) ~= "table" then return nil end
      value = value[part]
    end
    return value
  end

  local function invoke(root, path, ...)
    local value = resolve(root, path)
    if type(value) ~= "function" then return nil end
    local ok, result = pcall(value, ...)
    if ok then return result end
    return nil
  end

  local function snapshot(descriptor)
    if descriptor == nil then return nil end
    if descriptor.snapshot ~= nil then return descriptor.snapshot end
    local host = {
      now_ms = invoke(noctalia, "nowMs") or 0,
      dark = invoke(noctalia, "isDarkMode"),
      outputs = invoke(noctalia, "outputs"),
      focused_output_name = invoke(noctalia, "focusedOutputName"),
      pluginDir = invoke(noctalia, "pluginDir"),
      pluginDataDir = invoke(noctalia, "pluginDataDir"),
      clipboard = invoke(noctalia, "clipboardText"),
      calendar = invoke(noctalia, "calendarEvents"),
      media = invoke(noctalia, "mediaPlayer"),
      stats = invoke(noctalia, "systemStats"),
      nightlight = invoke(noctalia, "nightlight.state"),
      compositor = { name = invoke(noctalia, "compositor.name") },
      setting = {},
      bar = { vertical = invoke(barWidget, "isVertical"), output_name = invoke(barWidget, "outputName") },
      colors = {},
    }
    for _, path in ipairs(SETTING_PATHS) do host.setting[path] = invoke(noctalia, "getSetting", path) end
    for _, role in ipairs(COLOR_ROLES) do host.colors[role] = invoke(noctalia, "getColor", role) end
    local config = {}
    for _, key in ipairs(CONFIG_KEYS) do config[key] = noctalia.getConfig(key) end
    descriptor.snapshot = {
      entry = ENTRY,
      event = descriptor.event,
      payload = descriptor.payload or {},
      config = config,
      state = noctalia.state.get(STATE_KEY) or {},
      host = host,
    }
    return descriptor.snapshot
  end

  -- Volatile reads must not defeat the cache. Every other field matches the
  -- helper's key, which drops the same field.
  local function stable(context)
    local host = {}
    for key, value in pairs(context.host) do
      if key ~= "now_ms" then host[key] = value end
    end
    return {
      entry = context.entry,
      event = context.event,
      payload = context.payload,
      config = context.config,
      state = context.state,
      host = host,
    }
  end

  function M.call(name, args, cache_ms, needs_context, as_result)
    local context = nil
    if needs_context then context = snapshot(current) end
    local encoded = nil
    if cache_ms and cache_ms > 0 then
      encoded = name .. string.char(0) .. noctalia.json.encode(args)
      if context ~= nil then
        encoded = encoded .. string.char(0) .. noctalia.json.encode(stable(context))
      end
      local hit = cache[encoded]
      if hit and noctalia.nowMs() - hit.at < cache_ms then return hit.value end
    end
    local value, failure = coroutine.yield({
      kind = "native",
      name = name,
      args = args,
      cache_ms = cache_ms or 0,
      context = context,
    })
    if failure ~= nil then
      if as_result then return nil, tostring(failure) end
      error(tostring(failure), 0)
    end
    if encoded ~= nil then remember(encoded, value) end
    if as_result then return value, nil end
    return value
  end

  local function park(co, request)
    active = { co = co, request = request, context = current }
    M.send(request)
  end

  function M.pump()
    while active == nil and #queue > 0 do
      local job = table.remove(queue, 1)
      current = job.context
      local co = coroutine.create(job.fn)
      local ok, yielded = coroutine.resume(co, table.unpack(job.args))
      if not ok then error(yielded, 0) end
      if type(yielded) == "table" and yielded.kind == "native" then park(co, yielded) end
    end
  end

  function M.run(fn, context, ...)
    queue[#queue + 1] = { fn = fn, context = context, args = { ... } }
    M.pump()
  end

  local function on_line(line)
    if type(line) == "string" then
      local ok, decoded = pcall(noctalia.json.decode, line)
      if not ok or type(decoded) ~= "table" then return end
      line = decoded
    end
    if type(line) ~= "table" then return end
    if line.control == "ready" then
      boot = line.boot
      request_dir = line.spool
      local queued = pending
      pending = {}
      for _, request in ipairs(queued) do
        if not write(request) then pending[#pending + 1] = request end
      end
      if active ~= nil
        and active.request ~= nil
        and active.request.boot ~= boot then
        write(active.request)
      end
      return
    end
    if line.control ~= "native_result" or active == nil then return end
    if active.request ~= nil
      and active.request.id ~= nil
      and line.id ~= active.request.id then
      noctalia.log(
        "noctalia native: stale result " .. tostring(line.id)
        .. " for " .. tostring(active.request.id)
      )
      return
    end
    local co = active.co
    current = active.context
    active = nil
    local ok, yielded = coroutine.resume(co, line.value, line.error)
    if not ok then error(yielded, 0) end
    if type(yielded) == "table" and yielded.kind == "native" then
      park(co, yielded)
      return
    end
    M.pump()
  end

  function M.setup()
    if noctalia.mkdirAll then noctalia.mkdirAll(SPOOL_DIR) end
    local command = shellQuote(BIN) .. " serve " .. shellQuote(DATA_DIR) .. " " .. shellQuote(SPOOL_DIR)
    local loop = "while kill -0 \"$PPID\" 2>/dev/null; do " .. command .. "; sleep 0.2; done"
    if not noctalia.runStream(loop, on_line) then
      noctalia.log("noctalia native: could not start the persistent helper")
    end
  end

  M.setup()
  return M
end)()

"#,
    );
    out
}

fn lua_list(values: &[&str]) -> String {
    let items = values.iter().map(|value| quote(value)).collect::<Vec<_>>();
    format!("{{ {} }}", items.join(", "))
}

/// Parses `#[native]` and `#[native(cache_ms = 30_000)]`.
pub(super) fn forced_native(function: &ItemFn) -> Result<Option<u64>, Error> {
    for attr in &function.attrs {
        if !attr.path().is_ident("native") {
            continue;
        }
        return match &attr.meta {
            syn::Meta::Path(_) => Ok(Some(0)),
            syn::Meta::List(list) => {
                let mut cache_ms = 0u64;
                list.parse_nested_meta(|meta| {
                    if meta.path.is_ident("cache_ms") {
                        let value: syn::LitInt = meta.value()?.parse()?;
                        cache_ms = value.base10_parse::<u64>()?;
                        return Ok(());
                    }
                    Err(meta.error("unknown `#[native]` option"))
                })
                .map_err(|error| Error::at(error.span(), error.to_string()))?;
                Ok(Some(cache_ms))
            }
            syn::Meta::NameValue(_) => Err(Error::at(
                attr.span(),
                "`#[native]` takes `cache_ms` in parentheses",
            )),
        };
    }
    Ok(None)
}

fn is_luau_attr(function: &ItemFn) -> bool {
    function
        .attrs
        .iter()
        .any(|attr| attr.path().is_ident("luau"))
}

pub(super) fn native_reason(error: &Error) -> String {
    match &error.file {
        Some(file) if error.line != 0 => {
            format!("{file}:{}:{}: {}", error.line, error.column, error.message)
        }
        _ => error.message.clone(),
    }
}

/// Checks the JSON-crossable signature a native function needs.
fn native_signature(function: &ItemFn) -> Result<Vec<NativeParam>, Error> {
    let name = function.sig.ident.to_string();
    if !function.sig.generics.params.is_empty() {
        return Err(Error::at(
            function.sig.ident.span(),
            format!("native function `{name}` cannot be generic"),
        ));
    }
    for input in &function.sig.inputs {
        if let syn::FnArg::Receiver(receiver) = input {
            return Err(Error::at(
                receiver.span(),
                format!(
                    "native function `{name}` cannot take `self`; use an associated function"
                ),
            ));
        }
    }
    let mut params = Vec::new();
    for (pattern, ty) in typed_args(function) {
        let Pat::Ident(pattern) = pattern else {
            return Err(Error::at(
                pattern.span(),
                format!(
                    "native function `{name}` parameters must be plain identifiers"
                ),
            ));
        };
        if pattern.by_ref.is_some() || pattern.subpat.is_some() {
            return Err(Error::at(
                pattern.span(),
                format!(
                    "native function `{name}` parameters must be plain identifiers"
                ),
            ));
        }
        let param = native_param(pattern.ident.to_string(), ty, &name)?;
        params.push(param);
    }
    if let syn::ReturnType::Type(_, ty) = &function.sig.output {
        if matches!(&**ty, Type::ImplTrait(_)) {
            return Err(Error::at(
                ty.span(),
                format!(
                    "native function `{name}` must return a concrete value or `Result<T, E>`"
                ),
            ));
        }
        if is_result(ty) && generic_arg(ty).is_none() {
            return Err(Error::at(
                ty.span(),
                format!(
                    "native function `{name}` returns an incomplete `Result`"
                ),
            ));
        }
    }
    Ok(params)
}

#[derive(Clone, Copy)]
pub(crate) enum Direction {
    Serialize,
    Deserialize,
}

/// Conservative check that a type can cross the JSON boundary.
fn json_ready_type(
    program: &Program,
    module: &str,
    ty: &Type,
    direction: Direction,
) -> bool {
    pub(crate) const SCALARS: &[&str] = &[
        "String", "str", "bool", "char", "i8", "i16", "i32", "i64", "i128",
        "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32", "f64",
        "Value", "PathBuf", "Path", "OsString", "OsStr",
    ];
    pub(crate) const CONTAINERS: &[&str] = &[
        "Vec", "VecDeque", "Option", "HashMap", "BTreeMap", "HashSet",
        "BTreeSet", "Box", "Arc", "Rc", "Cow", "Result",
    ];
    match ty {
        Type::Path(path) => {
            let Some(segment) = path.path.segments.last() else {
                return false;
            };
            let name = segment.ident.to_string();
            if SCALARS.contains(&name.as_str()) {
                return true;
            }
            if CONTAINERS.contains(&name.as_str()) {
                let PathArguments::AngleBracketed(arguments) =
                    &segment.arguments
                else {
                    return name == "Vec" || name == "Option";
                };
                return arguments.args.iter().all(|argument| {
                    let syn::GenericArgument::Type(inner) = argument else {
                        return matches!(
                            argument,
                            syn::GenericArgument::Lifetime(_)
                        );
                    };
                    json_ready_type(program, module, inner, direction)
                });
            }
            program
                .struct_def(module, &name)
                .is_some_and(|def| match direction {
                    Direction::Serialize => def.derives_serialize,
                    Direction::Deserialize => def.derives_deserialize,
                })
        }
        Type::Slice(slice) => {
            json_ready_type(program, module, &slice.elem, direction)
        }
        Type::Tuple(tuple) => tuple
            .elems
            .iter()
            .all(|item| json_ready_type(program, module, item, direction)),
        Type::Paren(paren) => {
            json_ready_type(program, module, &paren.elem, direction)
        }
        Type::Group(group) => {
            json_ready_type(program, module, &group.elem, direction)
        }
        // A returned reference serializes; a deserialized one cannot.
        Type::Reference(reference)
            if matches!(direction, Direction::Serialize) =>
        {
            json_ready_type(program, module, &reference.elem, direction)
        }
        _ => false,
    }
}

/// Maps one declared parameter to the local the dispatch needs.
fn native_param(
    name: String,
    ty: &Type,
    function: &str,
) -> Result<NativeParam, Error> {
    let mut param = NativeParam {
        name,
        kind: NativeParamKind::Value,
        owned: None,
        pass: NativePass::Value,
    };
    if is_context(ty) {
        param.kind = NativeParamKind::Context;
        return Ok(param);
    }
    if matches!(ty, Type::ImplTrait(_)) {
        return Err(Error::at(
            ty.span(),
            format!(
                "native function `{function}` parameters cannot be `impl Trait`; name the type"
            ),
        ));
    }
    if let Type::Path(path) = ty
        && path
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "Option")
        && let Some(inner) = generic_arg(ty)
    {
        if let Type::Reference(reference) = inner {
            if reference.mutability.is_some() {
                return Err(mutable_reference(ty, function));
            }
            if matches!(&*reference.elem, Type::Reference(_)) {
                return Err(nested_reference(ty, function));
            }
            let (owned, pass) = optional_reference_local(&reference.elem);
            param.owned = owned.map(|ty| format!("Option<{ty}>"));
            param.pass = pass;
            return Ok(param);
        }
        return Ok(param);
    }
    let Type::Reference(reference) = ty else {
        return Ok(param);
    };
    if reference.mutability.is_some() {
        return Err(mutable_reference(ty, function));
    }
    if matches!(&*reference.elem, Type::Reference(_)) {
        return Err(nested_reference(ty, function));
    }
    let (owned, pass) = reference_local(&reference.elem);
    param.owned = owned;
    param.pass = pass;
    Ok(param)
}

fn mutable_reference(ty: &Type, function: &str) -> Error {
    Error::at(
        ty.span(),
        format!(
            "native function `{function}` cannot take `&mut` parameters; changes would be lost"
        ),
    )
}

fn nested_reference(ty: &Type, function: &str) -> Error {
    Error::at(
        ty.span(),
        format!("native function `{function}` cannot take nested references"),
    )
}

/// The owned local and pass mode for the inner `&T` of `Option<&T>`.
fn optional_reference_local(inner: &Type) -> (Option<String>, NativePass) {
    match inner {
        Type::Slice(_) => (Some("Vec<_>".to_owned()), NativePass::AsDeref),
        _ => match type_text(inner).as_str() {
            "str" | "Path" => {
                let owned = if type_text(inner) == "str" {
                    "String"
                } else {
                    "PathBuf"
                };
                (Some(owned.to_owned()), NativePass::AsDeref)
            }
            "String" => (Some("String".to_owned()), NativePass::AsRef),
            "PathBuf" => (Some("PathBuf".to_owned()), NativePass::AsRef),
            _ => (None, NativePass::AsRef),
        },
    }
}

/// The owned local and pass mode for a `&T` parameter.
fn reference_local(inner: &Type) -> (Option<String>, NativePass) {
    let owned = match inner {
        Type::Slice(_) => Some("Vec<_>".to_owned()),
        _ => match type_text(inner).as_str() {
            "str" => Some("String".to_owned()),
            "String" => Some("String".to_owned()),
            "Path" | "PathBuf" => Some("PathBuf".to_owned()),
            _ => None,
        },
    };
    let pass = match inner {
        Type::Slice(_) => NativePass::Slice,
        _ => NativePass::Ref,
    };
    (owned, pass)
}
