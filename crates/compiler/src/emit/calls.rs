use super::*;

impl<'a> Emitter<'a> {
    /// A `unwrap_or_else` fallback, which may be `Default::default` itself.
    pub(super) fn fallback(
        &mut self,
        expr: &Expr,
        ty: Ty,
    ) -> Result<(String, Ty), Error> {
        if is_default_path(expr) {
            let ty = Ty::new(ty.base);
            let literal = ty
                .default_literal()
                .ok_or_else(|| bare_default_error(expr.span()))?;
            return Ok((literal.to_owned(), ty));
        }
        self.invoke_arg(expr, &[])
    }

    /// `Type::default` used as a callback value, such as
    /// `get_or_insert_with(WidgetState::default)`.
    fn default_reference(
        &mut self,
        expr: &Expr,
        values: &[String],
    ) -> Result<OptionalValue, Error> {
        if !values.is_empty() {
            return Ok(None);
        }
        let Expr::Path(path) = strip_reference(expr) else {
            return Ok(None);
        };
        let Some(segments) = path_segments(&path.path) else {
            return Ok(None);
        };
        if segments.len() < 2
            || segments.last().is_none_or(|name| name != "default")
        {
            return Ok(None);
        }
        let class = segments[segments.len() - 2].clone();
        if let Some((module, function)) =
            self.program.resolve_method(&self.module, &class, "default")
        {
            let function = function.clone();
            let helper = Helper {
                module,
                name: format!("{class}::default"),
                function,
                class: Some(class.clone()),
            };
            let returns = return_kind(&helper.function)?;
            let check = self.check_inline(
                &helper,
                Inline::Value,
                returns,
                &expr.span(),
            )?;
            let (text, _) = self.inline(
                &helper,
                Vec::new(),
                Inline::Value,
                returns,
                check,
            )?;
            return Ok(Some((text, Ty::VALUE)));
        }
        let ty: Type = syn::parse_str(&class).map_err(|error| {
            Error::at(expr.span(), format!("invalid default type: {error}"))
        })?;
        let text = default_value(self, &ty, expr.span())?;
        Ok(Some((text, type_hint(&ty))))
    }

    /// Inlines a crate function used as a callback value, such as
    /// `map(lua_text)` or `unwrap_or_else(default_snapshot)`.
    pub(super) fn function_reference(
        &mut self,
        expr: &Expr,
        values: &[String],
    ) -> Result<OptionalValue, Error> {
        if let Some(value) = self.default_reference(expr, values)? {
            return Ok(Some(value));
        }
        let Expr::Path(path) = strip_reference(expr) else {
            return Ok(None);
        };
        let Some(segments) = path_segments(&path.path) else {
            return Ok(None);
        };
        let Some(resolved) = self.program.resolve(&self.module, &segments)
        else {
            return Ok(None);
        };
        let helper = Helper {
            module: resolved.module.clone(),
            name: segments.join("::"),
            function: resolved.function.clone(),
            class: None,
        };
        let params = typed_args(&helper.function);
        if params.len() != values.len() {
            return Err(Error::at(
                expr.span(),
                format!(
                    "`{}` takes {} arguments but {} were passed",
                    helper.name,
                    params.len(),
                    values.len()
                ),
            ));
        }
        let mut bindings = Vec::new();
        for ((pattern, _), value) in params.iter().zip(values) {
            let Pat::Ident(pattern) = pattern else {
                return Err(Error::at(
                    expr.span(),
                    "helper parameters must be plain identifiers",
                ));
            };
            bindings.push(Binding::Local {
                name: pattern.ident.to_string(),
                text: value.clone(),
                ty: Ty::VALUE,
                container: StaticKind::Value,
            });
        }
        self.inline_resolved(helper, Inline::Value, expr.span(), bindings)
    }

    pub(super) fn call(
        &mut self,
        call: &ExprCall,
    ) -> Result<(String, Ty), Error> {
        if let Some(name) = path_last_name(&call.func) {
            let args = call.args.iter().collect::<Vec<_>>();
            if !name.contains("::")
                && self.lookup(&name).is_some()
                && !self.is_ctx_expr(&call.func)
            {
                let mut values = Vec::new();
                for argument in &args {
                    values.push(self.expr(argument)?.0);
                }
                return Ok((
                    format!("{name}({})", values.join(", ")),
                    Ty::VALUE,
                ));
            }
            match name.as_str() {
                "Some" => return self.expr(arg(&args, 0, call)?),
                "drop" => {
                    let _ = self.expr(arg(&args, 0, call)?)?;
                    return Ok((String::new(), Ty::VOID));
                }
                "String::new" => return Ok(("\"\"".to_owned(), Ty::STR)),
                "Default::default" => {
                    return Err(bare_default_error(call.span()));
                }
                "Vec::new" | "VecDeque::new" => {
                    return Ok((empty_array(), Ty::LIST));
                }
                "Vec::with_capacity" | "VecDeque::with_capacity" => {
                    return Ok(("{}".to_owned(), Ty::LIST));
                }
                "String::with_capacity" => {
                    return Ok(("\"\"".to_owned(), Ty::STR));
                }
                "std::cmp::max" | "cmp::max" => {
                    let left = self.expr(arg(&args, 0, call)?)?.0;
                    let right = self.expr(arg(&args, 1, call)?)?.0;
                    return Ok((
                        format!("math.max({left}, {right})"),
                        Ty::VALUE,
                    ));
                }
                "std::cmp::min" | "cmp::min" => {
                    let left = self.expr(arg(&args, 0, call)?)?.0;
                    let right = self.expr(arg(&args, 1, call)?)?.0;
                    return Ok((
                        format!("math.min({left}, {right})"),
                        Ty::VALUE,
                    ));
                }
                "SystemTime::now" => {
                    return Ok(("noctalia.nowMs()".to_owned(), Ty::INT));
                }
                "ByteBuf::new" | "ByteBuf::from"
                    if self.strict() && args.len() == 1 =>
                {
                    return self.byte_string(arg(&args, 0, call)?);
                }
                _ => {}
            }
            if args.len() == 1
                && let Some(name) = path_last_name(&call.func)
                && self.program.is_newtype(&self.module, &name)
            {
                let value = self.expr(arg(&args, 0, call)?)?.0;
                self.pending_class = Some(name.to_owned());
                return Ok((value, Ty::VALUE));
            }
            let identity = name.ends_with("::try_from")
                || name.ends_with("::from_utf8_lossy")
                || name.ends_with("::from")
                || name.ends_with("::Borrowed")
                || name.ends_with("::uninit")
                || name == "serde_json::from_value"
                || name.ends_with("Value::Object")
                || name.ends_with("Value::Array")
                || name.ends_with("Value::String")
                || name.ends_with("Value::Bool")
                || name.ends_with("Value::Number")
                || name == "Arc::new";
            if identity && args.len() == 1 {
                let text = self.expr(arg(&args, 0, call)?)?.0;
                return Ok((text, Ty::VALUE));
            }
            if name == "env::var" || name == "std::env::var" {
                let key = self.expr(arg(&args, 0, call)?)?.0;
                return Ok((format!("noctalia.getenv({key})"), Ty::OPT_STR));
            }
            if name.ends_with("Number::from_f64") && args.len() == 1 {
                let value = self.expr(arg(&args, 0, call)?)?.0;
                return Ok((
                    format!(
                        "(function() local __value = {value}; if __value == __value and __value ~= math.huge and __value ~= -math.huge then return __value end end)()"
                    ),
                    Ty::OPT_FLOAT,
                ));
            }
            if let Some(class) = name.strip_suffix("::default")
                && class != "Default"
            {
                let class = if class == "Self" {
                    self.self_class.clone().unwrap_or_else(|| class.to_owned())
                } else {
                    class.to_owned()
                };
                let ty: Type = syn::parse_str(&class).map_err(|error| {
                    Error::at(
                        call.span(),
                        format!("invalid default type: {error}"),
                    )
                })?;
                if let Some((module, function)) =
                    self.program.resolve_method(&self.module, &class, "default")
                {
                    let helper = Helper {
                        module,
                        name: format!("{class}::default"),
                        function: function.clone(),
                        class: Some(class.to_owned()),
                    };
                    let returns = return_kind(&helper.function)?;
                    let check = self.check_inline(
                        &helper,
                        Inline::Value,
                        returns,
                        &call.span(),
                    )?;
                    return self.inline(
                        &helper,
                        Vec::new(),
                        Inline::Value,
                        returns,
                        check,
                    );
                }
                return Ok((default_value(self, &ty, call.span())?, Ty::VALUE));
            }
            if name == "from_str" || name == "serde_json::from_str" {
                let text = self.expr(arg(&args, 0, call)?)?.0;
                let base = json_parse_base(call)?;
                return Ok((format!("json.decode({text})"), Ty::result(base)));
            }
            if name == "serde_json::to_string" {
                let text = self.expr(arg(&args, 0, call)?)?.0;
                return Ok((format!("(json.encode({text}))"), Ty::STR));
            }
            if matches!(name.rsplit("::").next(), Some("from_iter"))
                && args.len() == 1
            {
                let list = arg(&args, 0, call)?;
                let Expr::Array(items) = strip_reference(list) else {
                    return Err(Error::at(
                        call.span(),
                        "from_iter() needs an array literal of pairs",
                    ));
                };
                if items
                    .elems
                    .iter()
                    .any(|item| !matches!(item, Expr::Tuple(tuple) if tuple.elems.len() == 2))
                {
                    return Err(Error::at(
                        call.span(),
                        "from_iter() needs an array literal of pairs",
                    ));
                }
                let list = self.expr(list)?.0;
                return Ok((
                    format!(
                        "(function() local __map = {{}} for _, __pair in ipairs({list}) do __map[__pair[1]] = __pair[2] end return __map end)()"
                    ),
                    Ty::TABLE,
                ));
            }
            if name.ends_with("::from_str_radix") {
                let text = self.expr(arg(&args, 0, call)?)?.0;
                let radix = self.expr(arg(&args, 1, call)?)?.0;
                return Ok((
                    format!("tonumber({text}, {radix})"),
                    Ty::result(Base::Int),
                ));
            }
            if let Some(variant) = qualified_variant(&name, "ui")
                && let Some(builder) = ui_marker(variant)
            {
                let argument = arg(&args, 0, call)?;
                if self.strict() {
                    return self.strict_ui_marker(
                        variant,
                        argument,
                        call.span(),
                    );
                }
                let value = self.expr(argument)?.0;
                return Ok((format!("{{ {builder} = {value} }}"), Ty::VALUE));
            }
            if let Some(value) = self.std_constructor(call, &args)? {
                return Ok(value);
            }
            if let Some(kind) = ui_constructor(&name) {
                return self.ui_node(call, kind);
            }
            if let Some(value) = self.associated_call(call, &args)? {
                return Ok(value);
            }
            if let Some(op) = op_constructor(&name) {
                self.op_call(call, &op)?;
                return Ok((String::new(), Ty::VOID));
            }
            if name == "Op::query" && self.strict() {
                return Err(Error::at(
                    call.span(),
                    "strict op `query` can only be pushed or returned as an Op",
                ));
            }
            if let Some(op) = unsupported_op(&name) {
                return Err(Error::at(call.span(), op));
            }
        }
        if let Some(value) = self.inline_call(call, Inline::Value)? {
            return Ok(value);
        }
        let label = path_last_name(&call.func).unwrap_or_default();
        Err(Error::at(
            call.span(),
            format!(
                "call to `{label}` is not supported by the Luau compiler: no function in the crate matches, and only ops, ui constructors and a small set of std calls are supported"
            ),
        ))
    }

    /// A byte string for a strict `ByteBuf`.
    ///
    /// A byte array becomes `string.char(...)` so it stays a Lua string. Any
    /// other expression (a `String`, `&str`, or a value already carrying
    /// bytes) is used as is.
    fn byte_string(&mut self, expr: &Expr) -> Result<(String, Ty), Error> {
        let inner = strip_reference(expr);
        let items = match inner {
            Expr::Macro(mac) if macro_name(mac) == "vec" => parse_vec(mac)?,
            Expr::Array(array) => {
                array.elems.iter().cloned().collect::<Vec<_>>()
            }
            _ => return self.expr(expr),
        };
        let mut values = Vec::new();
        let mut all_literals = true;
        for item in &items {
            let Expr::Lit(ExprLit {
                lit: Lit::Int(value),
                ..
            }) = strip_reference(item)
            else {
                all_literals = false;
                break;
            };
            values.push(value.base10_parse::<u32>()?.to_string());
        }
        if all_literals {
            return Ok((
                format!("string.char({})", values.join(", ")),
                Ty::STR,
            ));
        }
        let list = self.list(&items)?.0;
        self.uses_unpack = true;
        Ok((format!("string.char(__unpack({list}))"), Ty::STR))
    }

    /// A real callback value for strict emit.
    ///
    /// `ui::callback(id)` becomes a closure that forwards its arguments to the
    /// host-callback body, and `ui::call(name)` becomes the name of the plugin
    /// global emitted for that action. A dynamic action name becomes a
    /// closure that forwards to the shared action dispatcher.
    fn strict_ui_marker(
        &mut self,
        variant: &str,
        argument: &Expr,
        span: proc_macro2::Span,
    ) -> Result<(String, Ty), Error> {
        match variant {
            "callback" => {
                if !self.has_host_callback {
                    return Err(Error::at(
                        span,
                        "ui::callback needs a host_callback branch in strict mode",
                    ));
                }
                let id = self.expr(argument)?.0;
                self.uses_callback_dispatch = true;
                Ok((strict::closure(strict::CALLBACK_DISPATCH, &id), Ty::VALUE))
            }
            "call" => {
                if !self.has_action {
                    return Err(Error::at(
                        span,
                        "ui::call needs an action branch in strict mode",
                    ));
                }
                if let Some(name) = literal_string(argument) {
                    self.actions.insert(name.clone());
                    let global = strict::action_global(&name);
                    return Ok((format!("\"{global}\""), Ty::VALUE));
                }
                let name = self.expr(argument)?.0;
                self.uses_action_dispatch = true;
                Ok((strict::closure(strict::ACTION_DISPATCH, &name), Ty::VALUE))
            }
            _ => Err(Error::at(span, "unknown ui marker")),
        }
    }

    fn ui_node(
        &mut self,
        call: &ExprCall,
        kind: &str,
    ) -> Result<(String, Ty), Error> {
        let args = call.args.iter().collect::<Vec<_>>();
        let props = match args.first() {
            Some(expr) => self.expr(expr)?.0,
            None => "{}".to_owned(),
        };
        let children = match args.get(1) {
            Some(expr) => {
                let expr = strip_reference(expr);
                if let Expr::Call(vec) = expr
                    && is_vec_new(vec)
                {
                    "{}".to_owned()
                } else if let Expr::Macro(mac) = expr
                    && macro_name(mac) == "vec"
                {
                    let items = parse_vec(mac)?;
                    self.list(&items)?.0
                } else {
                    self.expr(expr)?.0
                }
            }
            None => "{}".to_owned(),
        };
        Ok((
            format!(
                "{{ kind = {}, props = {props}, children = {children} }}",
                quote(kind)
            ),
            Ty::NODE,
        ))
    }
}

pub(super) fn ui_constructor(name: &str) -> Option<&'static str> {
    let variant = qualified_variant(name, "ui")?;
    match variant {
        "column" => Some("column"),
        "row" => Some("row"),
        "scroll" => Some("scroll"),
        "label" => Some("label"),
        "button" => Some("button"),
        "input" => Some("input"),
        "progress" => Some("progress"),
        _ => None,
    }
}

/// The marker key for `ui::call` and `ui::callback`.
fn ui_marker(variant: &str) -> Option<&'static str> {
    match variant {
        "call" => Some("__call"),
        "callback" => Some("__host_callback"),
        _ => None,
    }
}

/// A string literal, optionally behind a reference.
pub(super) fn literal_string(expr: &Expr) -> Option<String> {
    let expr = strip_reference(expr);
    let Expr::Lit(ExprLit {
        lit: Lit::Str(value),
        ..
    }) = expr
    else {
        return None;
    };
    Some(value.value())
}
