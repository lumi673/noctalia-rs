use super::*;

impl<'a> Emitter<'a> {
    pub(super) fn list(
        &mut self,
        items: &[Expr],
    ) -> Result<(String, Ty), Error> {
        if items.is_empty() {
            return Ok((empty_array(), Ty::LIST));
        }
        let mut values = Vec::new();
        for item in items {
            let (text, item_ty) = self.expr(item)?;
            if text.is_empty() {
                continue;
            }
            reject_null(
                &text,
                item_ty,
                item.span(),
                "a list literal",
                self.strict(),
            )?;
            values.push(text);
        }
        Ok((format!("{{ {} }}", values.join(", ")), Ty::LIST))
    }

    pub(super) fn expr(&mut self, expr: &Expr) -> Result<(String, Ty), Error> {
        let result = self.expr_inner(expr);
        if let Ok((text, ty)) = &result
            && ty.null
        {
            self.null_texts.insert(text.clone());
        }
        result
    }

    fn expr_inner(&mut self, expr: &Expr) -> Result<(String, Ty), Error> {
        match expr {
            Expr::Lit(lit) => self.literal(lit),
            Expr::Path(path) => self.path(expr, path),
            Expr::Let(item) => {
                let condition = self.let_condition(item)?;
                Ok((condition, Ty::BOOL))
            }
            Expr::Paren(paren) => self.expr(&paren.expr),
            Expr::Group(group) => self.expr(&group.expr),
            Expr::Reference(reference) => self.expr(&reference.expr),
            Expr::Unary(unary) => self.unary(unary),
            Expr::Binary(binary)
                if matches!(binary.op, BinOp::And(_))
                    && and_let_count(&Expr::Binary(binary.clone())) > 1 =>
            {
                self.chain_condition(binary)
            }
            Expr::Binary(binary) => self.binary(binary),
            Expr::MethodCall(call)
                if call.method == "collect"
                    && call.turbofish.as_ref().is_some_and(|generic| {
                        generic.args.iter().any(|argument| {
                            matches!(
                                argument,
                                syn::GenericArgument::Type(Type::Path(path))
                                    if path.path.segments.last().is_some_and(
                                        |segment| segment.ident == "String"
                                    )
                            )
                        })
                    }) =>
            {
                let (text, ty) = self.expr(&call.receiver)?;
                if ty.base == Base::Str {
                    return Ok((text, ty));
                }
                if self.is_chars(&text) {
                    return Ok((
                        format!(
                            "table.concat({text}, \"\", ({text}.__next or 1))"
                        ),
                        Ty::STR,
                    ));
                }
                Ok((format!("table.concat({text})"), Ty::STR))
            }
            Expr::MethodCall(call) if call.method == "chars" => {
                if matches!(strip_reference(&call.receiver), Expr::Block(_)) {
                    return Err(Error::at(
                        call.span(),
                        "block receivers for `chars()` are not supported by the Luau compiler",
                    ));
                }
                let (source, ty) = self.expr(&call.receiver)?;
                if ty.base != Base::Str {
                    return self.method(call);
                }
                let value = self.temp();
                self.line(&format!("local {value} = {source}"));
                let chars = self.temp();
                self.line(&format!(
                    "local {chars} = (function() local __parts = {{}}; for __position, __codepoint in utf8.codes({value}) do __parts[#__parts + 1] = utf8.char(__codepoint) end; __parts.__noctalia_chars = true; __parts.__next = 1; __parts.__byte_cursor = 1; __parts.__source = {value}; return __parts end)()"
                ));
                self.mark_chars(&chars);
                Ok((chars, Ty::LIST))
            }
            Expr::MethodCall(call)
                if matches!(
                    call.method.to_string().as_str(),
                    "next" | "skip" | "take"
                ) =>
            {
                if !self.receiver_is_chars(&call.receiver) {
                    return self.method(call);
                }
                let text = self.expr(&call.receiver)?.0;
                let args = call.args.iter().collect::<Vec<_>>();
                let method = call.method.to_string();
                let body = if method == "next" {
                    format!(
                        "(function() local __list = {text}; local __position = __list.__next or 1; local __value = __list[__position]; if __value ~= nil then __list.__next = __position + 1; __list.__byte_cursor = (__list.__byte_cursor or 1) + #__value end; return __value end)()"
                    )
                } else {
                    let count = self.expr(arg(&args, 0, call)?)?.0;
                    let bounded =
                        format!("math.min(#__list, math.max(0, {count}))");
                    let start = if method == "skip" {
                        format!("(__list.__next or 1) + {bounded}")
                    } else {
                        "(__list.__next or 1)".to_owned()
                    };
                    let limit = if method == "take" {
                        format!("math.min(#__list, ({start}) + {bounded} - 1)")
                    } else {
                        "#__list".to_owned()
                    };
                    format!(
                        "(function() local __list = {text}; local __out = {{}}; local __byte = 1; for __i = 1, ({start}) - 1 do __byte = __byte + #(__list[__i] or \"\") end; for __i = {start}, {limit} do __out[#__out + 1] = __list[__i] end; __out.__noctalia_chars = true; __out.__next = 1; __out.__byte_cursor = 1; __out.__source = string.sub(__list.__source, __byte); return __out end)()"
                    )
                };
                let value = self.temp();
                self.line(&format!("local {value} = {body}"));
                if method != "next" {
                    self.mark_chars(&value);
                }
                Ok((
                    value,
                    if method == "next" {
                        Ty::OPT_STR
                    } else {
                        Ty::LIST
                    },
                ))
            }
            Expr::MethodCall(call) if call.method == "as_str" => {
                let (text, _) = self.expr(&call.receiver)?;
                if !self.is_chars(&text) {
                    return self.method(call);
                }
                Ok((
                    format!(
                        "string.sub({text}.__source, {text}.__byte_cursor or 1)"
                    ),
                    Ty::STR,
                ))
            }
            Expr::MethodCall(call) => self.method(call),
            Expr::Call(call) => match self.std_call(call)? {
                Some(value) => Ok(value),
                None => self.call(call),
            },
            Expr::Field(field) => self.field(expr, field),
            Expr::Macro(mac) => self.macro_expr(mac),
            Expr::Index(index) => self.index(index),
            Expr::Cast(cast) => self.cast(cast),
            Expr::Try(item) => self.try_expr(&item.expr),
            Expr::Block(block) => {
                self.push_scope();
                self.local_statics.push(HashMap::new());
                let result = self.block_expr(&block.block);
                let chars = match &result {
                    Ok((text, _)) => self.is_chars(text),
                    Err(_) => false,
                };
                self.local_statics.pop();
                self.pop_scope();
                if chars && let Ok((text, _)) = &result {
                    self.mark_chars(text);
                }
                result
            }
            Expr::Repeat(repeat) => {
                let len = match &*repeat.len {
                    Expr::Lit(ExprLit {
                        lit: Lit::Int(value),
                        ..
                    }) => value.base10_parse::<usize>().map_err(|error| {
                        Error::at(
                            repeat.len.span(),
                            format!("invalid array length: {error}"),
                        )
                    })?,
                    other => {
                        return Err(Error::at(
                            other.span(),
                            "array length must be a literal",
                        ));
                    }
                };
                let (value, _) = self.expr(&repeat.expr)?;
                Ok((format!("{{ {} }}", vec![value; len].join(", ")), Ty::LIST))
            }
            Expr::Closure(item) => self.closure(item),
            Expr::Match(item) => {
                let saved_out = std::mem::take(&mut self.out);
                let saved_indent = self.indent;
                let saved_target = self.tail_target.clone();
                self.indent = 0;
                self.tail_target = None;
                let result = self.match_stmt(item, true);
                let body = std::mem::replace(&mut self.out, saved_out);
                self.indent = saved_indent;
                self.tail_target = saved_target;
                result?;
                let ty = if match_returns_string(item) {
                    Ty::STR
                } else {
                    Ty::VALUE
                };
                Ok((format!("(function()\n{body}end)()"), ty))
            }
            Expr::Struct(item) => self.struct_expr(item),
            Expr::If(item) => {
                let (chain, else_branch) = if_chain(item);
                if chain
                    .iter()
                    .all(|(condition, _)| self.const_bool(condition).is_some())
                {
                    for (condition, block) in &chain {
                        if self.const_bool(condition) == Some(true) {
                            return self.block_value(block);
                        }
                    }
                    return match else_branch {
                        Some(Expr::Block(block)) => {
                            self.block_value(&block.block)
                        }
                        _ => Ok(("nil".to_owned(), Ty::VALUE)),
                    };
                }
                let name = self.temp();
                self.line(&format!("local {name}"));
                let condition = match strip_reference(&item.cond) {
                    Expr::Let(condition) => self.let_condition(condition)?,
                    other => self.expr(other)?.0,
                };
                self.line(&format!("if {condition} then"));
                self.indent = self.indent.saturating_add(1);
                let (value, then_ty) = self.block_value(&item.then_branch)?;
                let mut then_chars = self.is_chars(&value);
                let mut then_shapes =
                    self.tuple_expr_shapes.get(&value).cloned();
                let mut then_types = self.tuple_types.get(&value).cloned();
                let mut result = then_ty;
                if !value.is_empty() && then_ty.base != Base::Void {
                    self.line(&format!("{name} = {value}"));
                } else {
                    result = Ty::VALUE;
                }
                self.indent = self.indent.saturating_sub(1);
                if let Some((_, branch)) = &item.else_branch {
                    self.line("else");
                    self.indent = self.indent.saturating_add(1);
                    let (value, else_ty) = match &**branch {
                        Expr::Block(block) => self.block_value(&block.block)?,
                        other => self.expr(other)?,
                    };
                    then_chars = then_chars && self.is_chars(&value);
                    let else_shapes =
                        self.tuple_expr_shapes.get(&value).cloned();
                    let else_types = self.tuple_types.get(&value).cloned();
                    if then_shapes != else_shapes || then_types != else_types {
                        then_shapes = None;
                        then_types = None;
                    }
                    result.null = then_ty.null || else_ty.null;
                    result.absent = then_ty.absent && else_ty.absent;
                    result.optional = then_ty.optional || else_ty.optional;
                    if !value.is_empty() && else_ty.base != Base::Void {
                        self.line(&format!("{name} = {value}"));
                    } else {
                        // A branch that produces nothing leaves nil.
                        result = Ty::NULL;
                    }
                    self.indent = self.indent.saturating_sub(1);
                } else {
                    then_chars = false;
                    result.null = true;
                }
                if let (Some(shapes), Some(types)) = (then_shapes, then_types) {
                    self.tuple_expr_shapes.insert(name.clone(), shapes);
                    self.tuple_types.insert(name.clone(), types);
                }
                if then_chars {
                    self.mark_chars(&name);
                }
                self.line("end");
                Ok((name, result))
            }
            Expr::Array(array) => {
                let mut values = Vec::new();
                for item in &array.elems {
                    let (text, item_ty) = self.expr(item)?;
                    reject_null(
                        &text,
                        item_ty,
                        item.span(),
                        "an array literal",
                        self.strict(),
                    )?;
                    if !text.is_empty() {
                        values.push(text);
                    }
                }
                Ok((format!("{{ {} }}", values.join(", ")), Ty::LIST))
            }
            Expr::Tuple(tuple) if tuple.elems.is_empty() => {
                Ok(("nil".to_owned(), Ty::VOID))
            }
            Expr::Tuple(tuple) => {
                let mut values = Vec::new();
                let mut types = Vec::new();
                for element in &tuple.elems {
                    let (text, ty) = self.expr(element)?;
                    types.push(TupleShape::Leaf(ty));
                    values.push(text);
                }
                let text = format!("{{ {} }}", values.join(", "));
                self.tuple_expr_shapes.insert(text.clone(), types.clone());
                self.tuple_types.insert(text.clone(), types);
                Ok((text, Ty::LIST))
            }
            other => Err(Error::at(
                other.span(),
                "unsupported expression in a compilable handler",
            )),
        }
    }

    fn struct_expr(
        &mut self,
        item: &syn::ExprStruct,
    ) -> Result<(String, Ty), Error> {
        let segments = item
            .path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect::<Vec<_>>();
        if segments
            .get(segments.len().saturating_sub(2))
            .is_some_and(|name| name == "Op")
        {
            self.op_expr(&Expr::Struct(item.clone()))?;
            return Ok((String::new(), Ty::VOID));
        }
        let class = segments.last().cloned().unwrap_or_default();
        let class = if class == "Self" {
            self.self_class.clone().unwrap_or(class)
        } else {
            class
        };
        let mut fields: BTreeMap<String, String> = BTreeMap::new();
        let mut copied = None;
        if let Some(rest) = &item.rest {
            let is_default = matches!(&**rest, Expr::Call(call)
                if path_last_name(&call.func).as_deref() == Some("Default::default"));
            if is_default {
                let def = self
                    .program
                    .struct_def(&self.module, &class)
                    .ok_or_else(|| {
                        Error::at(
                            item.path.span(),
                            format!("unknown struct `{class}`"),
                        )
                    })?;
                for (name, ty) in &def.fields {
                    fields.insert(
                        name.clone(),
                        default_value(self, ty, item.path.span())?,
                    );
                }
            } else {
                let (base, _) = self.expr(rest)?;
                let target = self.temp();
                self.line(&format!("local {target} = {{}}"));
                self.line(&format!(
                    "for __k, __v in pairs({base}) do {target}[__k] = __v end"
                ));
                copied = Some(target);
            }
        }
        let mut null_fields = BTreeSet::new();
        for field in &item.fields {
            let syn::Member::Named(name) = &field.member else {
                return Err(Error::at(
                    field.member.span(),
                    "tuple structs are not supported by the Luau compiler",
                ));
            };
            let (value, value_ty) = self.expr(&field.expr)?;
            if let Some(shapes) = self.tuple_expr_shapes.get(&value).cloned() {
                self.tuple_expr_shapes
                    .insert(format!("{class}.{name}"), shapes);
            }
            if value_ty.null {
                null_fields.insert(name.to_string());
            }
            if let Some(target) = &copied {
                self.line(&format!(
                    "{target}.{} = {value}",
                    lua_key(&name.to_string())
                ));
            } else {
                fields.insert(name.to_string(), value);
            }
        }
        self.pending_class = Some(class);
        if !null_fields.is_empty() {
            self.pending_null_fields = Some(null_fields);
        }
        if let Some(target) = copied {
            return Ok((target, Ty::VALUE));
        }
        let fields = fields
            .into_iter()
            .map(|(name, value)| format!("{} = {value}", lua_key(&name)))
            .collect::<Vec<_>>();
        Ok((format!("{{ {} }}", fields.join(", ")), Ty::VALUE))
    }

    fn block_expr(
        &mut self,
        block: &syn::Block,
    ) -> Result<(String, Ty), Error> {
        let Some((last, rest)) = block.stmts.split_last() else {
            return Ok(("nil".to_owned(), Ty::VALUE));
        };
        for stmt in rest {
            self.stmt(stmt)?;
        }
        match last {
            Stmt::Expr(expr, _) => self.expr(expr),
            _ => Ok(("nil".to_owned(), Ty::VALUE)),
        }
    }
    fn std_call(&mut self, call: &ExprCall) -> Result<OptionalValue, Error> {
        let Some(name) = path_last_name(&call.func) else {
            return Ok(None);
        };
        if !name.contains("::") {
            let found = self.lookup(&name).is_some()
                || self
                    .program
                    .resolve(&self.module, std::slice::from_ref(&name))
                    .is_some();
            if found {
                return Ok(None);
            }
        }
        if let Expr::Path(path) = strip_reference(&call.func)
            && let Some(segments) = path_segments(&path.path)
            && self.program.resolve(&self.module, &segments).is_some()
        {
            return Ok(None);
        }
        if name == "std::str::from_utf8" || name == "std::str::from_utf8_lossy"
        {
            return Err(Error::at(
                call.span(),
                "UTF-8 conversion from bytes is not supported by the Luau compiler",
            ));
        }
        let args = call.args.iter().collect::<Vec<_>>();
        if matches!(
            name.as_str(),
            "Box::new" | "Arc::new" | "std::sync::Arc::new" | "Arc::clone"
        ) && args.len() == 1
        {
            return Ok(Some((self.expr(arg(&args, 0, call)?)?.0, Ty::VALUE)));
        }
        if name == "std::time::SystemTime::now" && args.is_empty() {
            return Ok(Some(("noctalia.nowMs()".to_owned(), Ty::INT)));
        }
        if name == "PathBuf::new" && args.is_empty() {
            return Ok(Some(("\"\"".to_owned(), Ty::STR)));
        }
        if name == "OpenOptions::new" && args.is_empty() {
            return Ok(Some(("{}".to_owned(), Ty::VALUE)));
        }
        let last = name.rsplit("::").next().unwrap_or(&name);
        let ui_import = name == "callback"
            || name == "call"
            || name == "node"
            || name.contains("::ui::");
        if name == "ByteBuf::new"
            && !self.strict()
            && args.len() == 1
            && matches!(
                strip_reference(arg(&args, 0, call)?),
                Expr::Macro(_) | Expr::Array(_)
            )
        {
            return Ok(Some((self.expr(arg(&args, 0, call)?)?.0, Ty::VALUE)));
        }
        if ui_import && matches!(last, "callback" | "call") && args.len() == 1 {
            if self.strict() {
                let mut strict_call = call.clone();
                strict_call.func = if last == "call" {
                    syn::parse_quote!(ui::call)
                } else {
                    syn::parse_quote!(ui::callback)
                };
                return Ok(Some(self.call(&strict_call)?));
            }
            let value = self.expr(arg(&args, 0, call)?)?.0;
            let marker = if last == "call" {
                "__call"
            } else {
                "__host_callback"
            };
            return Ok(Some((format!("{{ {marker} = {value} }}"), Ty::VALUE)));
        }
        if ui_import && last == "node" && !args.is_empty() {
            let kind = self.expr(arg(&args, 0, call)?)?.0;
            let props = match args.get(1) {
                Some(props) => self.expr(props)?.0,
                None => "{}".to_owned(),
            };
            let children = match args.get(2) {
                Some(children) => {
                    let children = strip_reference(children);
                    if matches!(children, Expr::Call(call) if is_vec_new(call))
                        || matches!(children, Expr::Macro(mac) if macro_name(mac) == "vec"
                            && parse_vec(mac).is_ok_and(|items| items.is_empty()))
                    {
                        empty_array()
                    } else {
                        self.expr(children)?.0
                    }
                }
                None => empty_array(),
            };
            return Ok(Some((
                format!(
                    "{{ kind = {kind}, props = {props}, children = {children} }}"
                ),
                Ty::NODE,
            )));
        }
        let numeric = ["f64", "f32", "i64", "u64", "usize", "i32", "u32"];
        if (name.ends_with("::max") || name.ends_with("::min"))
            && numeric
                .iter()
                .any(|kind| name.starts_with(&format!("{kind}::")))
            && args.len() == 2
        {
            let left = self.expr(arg(&args, 0, call)?)?.0;
            let right = self.expr(arg(&args, 1, call)?)?.0;
            let function = if name.ends_with("::max") {
                "max"
            } else {
                "min"
            };
            let text = if name.starts_with("f64::") || name.starts_with("f32::")
            {
                format!(
                    "(function() local __a, __b = {left}, {right}; if __a ~= __a then return __b end; if __b ~= __b then return __a end; return math.{function}(__a, __b) end)()"
                )
            } else {
                format!("math.{function}({left}, {right})")
            };
            let ty = if name.starts_with("f64::") || name.starts_with("f32::") {
                Ty::FLOAT
            } else {
                Ty::INT
            };
            return Ok(Some((text, ty)));
        }
        Ok(None)
    }

    fn block_value(
        &mut self,
        block: &syn::Block,
    ) -> Result<(String, Ty), Error> {
        let value = self.temp();
        self.line(&format!("local {value} = nil"));
        self.line("do");
        self.indent = self.indent.saturating_add(1);
        self.push_scope();
        let result = match block.stmts.split_last() {
            Some((last, rest)) => {
                for stmt in rest {
                    self.stmt(stmt)?;
                }
                match last {
                    Stmt::Expr(expr, _) => self.expr(expr),
                    _ => Ok(("nil".to_owned(), Ty::VALUE)),
                }
            }
            None => Ok(("nil".to_owned(), Ty::VALUE)),
        };
        let (text, ty) = result?;
        let chars = self.is_chars(&text);
        let shapes = self.tuple_expr_shapes.get(&text).cloned();
        let tuple_types = self.tuple_types.get(&text).cloned();
        if !text.is_empty() {
            self.line(&format!("{value} = {text}"));
        }
        if let Some(shapes) = shapes {
            self.tuple_expr_shapes.insert(value.clone(), shapes);
        }
        if let Some(types) = tuple_types {
            self.tuple_types.insert(value.clone(), types);
        }
        self.pop_scope();
        self.indent = self.indent.saturating_sub(1);
        self.line("end");
        if chars {
            self.mark_chars(&value);
        }
        Ok((value, ty))
    }

    pub(super) fn literal(
        &mut self,
        lit: &ExprLit,
    ) -> Result<(String, Ty), Error> {
        match &lit.lit {
            Lit::Str(value) => Ok((quote(&value.value()), Ty::STR)),
            Lit::Int(value) => Ok((value.base10_digits().to_owned(), Ty::INT)),
            Lit::Float(value) => {
                Ok((value.base10_digits().to_owned(), Ty::FLOAT))
            }
            Lit::Bool(value) => Ok((
                if value.value { "true" } else { "false" }.to_owned(),
                Ty::BOOL,
            )),
            Lit::Char(value) => {
                Ok((quote(&value.value().to_string()), Ty::STR))
            }
            Lit::Byte(value) => Ok((value.value().to_string(), Ty::INT)),
            Lit::ByteStr(value) => Err(Error::at(
                value.span(),
                "byte strings are not supported by the Luau compiler",
            )),
            other => Err(Error::at(other.span(), "unsupported literal")),
        }
    }

    pub(super) fn path(
        &mut self,
        expr: &Expr,
        path: &syn::ExprPath,
    ) -> Result<(String, Ty), Error> {
        if path.qself.is_none()
            && path.path.segments.len() > 1
            && let Some(segments) = path_segments(&path.path)
            && let Some(value) =
                self.program.resolve_constant(&self.module, &segments)
            && let Some((text, ty)) = constant(value)
        {
            return Ok((text, ty));
        }
        if let Some(segments) = path_segments(&path.path)
            && segments.last().is_some_and(|name| name == "from")
            && segments.iter().any(|name| name == "PathBuf")
        {
            return Ok((
                "(function(__value) return __value end)".to_owned(),
                Ty::VALUE,
            ));
        }
        if let Some(segments) = path_segments(&path.path)
            && segments.last().is_some_and(|name| name == "Null")
            && segments.iter().any(|name| name == "Value")
        {
            return Ok(("nil".to_owned(), Ty::NULL));
        }
        if let Some(segments) = path_segments(&path.path)
            && let Some(variant) =
                self.program.enum_variant(&self.module, &segments)
        {
            return Ok((quote(&variant), Ty::STR));
        }
        if let Some(segments) = path_segments(&path.path)
            && let Some(name) = segments.last()
        {
            if let Some((lua, _)) = self
                .local_statics
                .iter()
                .rev()
                .find_map(|scope| scope.get(name))
            {
                return Ok((lua.clone(), Ty::VALUE));
            }
            let resolved = self
                .program
                .resolve_static(&self.module, &segments)
                .map(|(module, _)| module);
            if let Some(module) = resolved {
                let def = self.register_static(&module, name)?;
                return Ok((def.name, Ty::VALUE));
            }
        }
        if let Some(segments) = path_segments(&path.path)
            && segments.iter().any(|name| name == "Value")
            && let Some(name) = segments.last()
            && matches!(
                name.as_str(),
                "String" | "Number" | "Bool" | "Array" | "Object"
            )
        {
            return Ok((
                "(function(__value) return __value end)".to_owned(),
                Ty::VALUE,
            ));
        }
        if let Some(segments) = path_segments(&path.path)
            && segments.iter().any(|name| name == "Ordering")
            && let Some(last) = segments.last()
            && matches!(
                last.as_str(),
                "Relaxed" | "Acquire" | "Release" | "AcqRel" | "SeqCst"
            )
        {
            return Ok((quote(last), Ty::STR));
        }
        if let Some(segments) = path_segments(&path.path)
            && segments.iter().any(|name| name == "Value")
            && let Some((lua_type, ty)) =
                segments.last().and_then(|name| value_accessor(name))
        {
            return Ok((
                format!(
                    "(function(__value) if type(__value) == \"{lua_type}\" then return __value end end)"
                ),
                ty,
            ));
        }
        if let Some(segments) = path_segments(&path.path) {
            let last = segments.last().map_or("", String::as_str);
            match last {
                "MAIN_SEPARATOR" | "MAIN_SEPARATOR_STR" => {
                    return Ok(("\"/\"".to_owned(), Ty::STR));
                }
                "PI" if segments.iter().any(|name| name == "consts") => {
                    return Ok(("math.pi".to_owned(), Ty::FLOAT));
                }
                "INFINITY" if segments.iter().any(|name| name == "f64") => {
                    return Ok(("math.huge".to_owned(), Ty::FLOAT));
                }
                "NEG_INFINITY" if segments.iter().any(|name| name == "f64") => {
                    return Ok(("(-math.huge)".to_owned(), Ty::FLOAT));
                }
                "NAN" if segments.iter().any(|name| name == "f64") => {
                    return Ok(("(0/0)".to_owned(), Ty::FLOAT));
                }
                "to_owned" | "to_string" | "as_slice" | "into_inner" => {
                    return Ok((
                        "(function(__value) return __value end)".to_owned(),
                        Ty::VALUE,
                    ));
                }
                "len"
                    if segments
                        .iter()
                        .any(|name| name == "Vec" || name == "String") =>
                {
                    return Ok((
                        "(function(__value) return #__value end)".to_owned(),
                        Ty::VALUE,
                    ));
                }
                _ => {}
            }
            if segments.first().is_some_and(|name| name == "ui")
                && let Some(kind) = segments
                    .last()
                    .and_then(|name| ui_constructor(&format!("ui::{name}")))
            {
                return Ok((
                    format!(
                        "(function(__props, __children) return {{ kind = {}, props = __props, children = __children or {{}} }} end)",
                        quote(kind)
                    ),
                    Ty::VALUE,
                ));
            }
        }
        if let Some(segments) = path_segments(&path.path) {
            let last = segments.last().map_or("", String::as_str);
            if let Some(kind) = segments
                .iter()
                .find(|name| name.as_str() == "f64" || name.as_str() == "f32")
                && let Some(value) = float_bound(kind.as_str(), last)
            {
                return Ok((value.to_owned(), Ty::FLOAT));
            }
            if let Some(lua_type) =
                segments.last().and_then(|name| value_predicate(name))
            {
                return Ok((
                    format!(
                        "(function(__value) return type(__value) == \"{lua_type}\" end)"
                    ),
                    Ty::VALUE,
                ));
            }
            if let Some((text, ty)) = constant(expr) {
                return Ok((text, ty));
            }
            if last == "is_empty" {
                return Ok((
                    "(function(__value) return #__value == 0 end)".to_owned(),
                    Ty::VALUE,
                ));
            }
            if last == "is_null" {
                return Ok((
                    "(function(__value) return __value == nil end)".to_owned(),
                    Ty::BOOL,
                ));
            }
            if matches!(
                last,
                "as_str"
                    | "as_bool"
                    | "as_f64"
                    | "as_i64"
                    | "as_u64"
                    | "as_array"
                    | "as_object"
            ) && segments
                .iter()
                .any(|name| matches!(name.as_str(), "Value" | "String" | "str"))
            {
                return Ok((
                    "(function(__value) return __value end)".to_owned(),
                    Ty::VALUE,
                ));
            }
            let math_call = match last {
                "floor" => Some("math.floor"),
                "ceil" => Some("math.ceil"),
                "round" => Some("math.floor(__value + 0.5)"),
                "abs" => Some("math.abs"),
                _ => None,
            };
            if let Some(function) = math_call
                && segments.len() > 1
                && segments.iter().any(|name| {
                    matches!(
                        name.as_str(),
                        "f64" | "f32" | "i64" | "u64" | "usize" | "i32"
                    )
                })
            {
                let body = if function.contains("__value") {
                    function.to_owned()
                } else {
                    format!("{function}(__value)")
                };
                return Ok((
                    format!("(function(__value) return {body} end)"),
                    Ty::VALUE,
                ));
            }
            if matches!(last, "new" | "default")
                && segments.len() > 1
                && container_default(&segments[segments.len() - 2]).is_some()
            {
                let literal = container_default(&segments[segments.len() - 2])
                    .unwrap_or_default();
                return Ok((
                    format!("(function() return {literal} end)"),
                    Ty::VALUE,
                ));
            }
            if last == "default"
                && segments.iter().any(|name| name == "Default")
            {
                return Ok((
                    "(function() return {} end)".to_owned(),
                    Ty::VALUE,
                ));
            }
            if segments.len() >= 2
                && let Some(value) =
                    self.program.resolve_constant(&self.module, &segments)
            {
                if let Some((text, ty)) = constant(value) {
                    return Ok((text, ty));
                }
                return Err(Error::at(
                    expr.span(),
                    "imported constant expression cannot be lowered in the handler scope",
                ));
            }
            if segments.len() == 2
                && segments.last().is_some_and(|name| name == "KEY")
                && let Some(def) = self
                    .program
                    .state_def(&self.module, &segments[segments.len() - 2])
            {
                return Ok((quote(&def.key), Ty::STR));
            }
            if segments.len() >= 2
                && let Some(name) = segments.last()
                && let Some(value) = self.program.associated_const(
                    &self.module,
                    &segments[segments.len() - 2],
                    name,
                )
            {
                if let Some((text, ty)) = constant(value) {
                    return Ok((text, ty));
                }
                let value = value.clone();
                let text = self.static_value(&value)?;
                return Ok((text, Ty::VALUE));
            }
        }
        let segments = &path.path.segments;
        let bare_none = segments.len() == 1
            && segments
                .first()
                .is_some_and(|segment| segment.ident == "None");
        let qualified_none = self.strict()
            && segments.len() == 2
            && segments
                .first()
                .is_some_and(|segment| segment.ident == "Option")
            && segments
                .last()
                .is_some_and(|segment| segment.ident == "None");
        if bare_none || qualified_none {
            let ty = if self.strict() { Ty::ABSENT } else { Ty::NULL };
            return Ok(("nil".to_owned(), ty));
        }
        let Some(name) = path.path.get_ident() else {
            return Err(Error::at(expr.span(), "unsupported path expression"));
        };
        let name = name.to_string();
        if name == "true" || name == "false" {
            return Ok((name, Ty::BOOL));
        }
        if name == "None" {
            let ty = if self.strict() { Ty::ABSENT } else { Ty::NULL };
            return Ok(("nil".to_owned(), ty));
        }
        if name == self.ctx {
            return Err(Error::at(
                expr.span(),
                "the context must be accessed through a supported field or method",
            ));
        }
        if self.lookup(&name) == Some(Ty::CONTEXT) {
            return Err(Error::at(
                expr.span(),
                "the context can only be passed to an inlined helper",
            ));
        }
        if let Some(ty) = self.lookup(&name) {
            return Ok((self.local_name(&name), ty));
        }
        if let Some((value, ty)) = self.constant(&name) {
            return Ok((value, ty));
        }
        if let Some(value) = self.program.imported_constant(&self.module, &name)
            && let Some((text, ty)) = constant(value)
        {
            return Ok((text, ty));
        }
        if let Some(value) = self.program.imported_constant(&self.module, &name)
        {
            let value = value.clone();
            return Ok((self.static_value(&value)?, Ty::VALUE));
        }
        Err(Error::at(
            expr.span(),
            format!("unknown variable `{name}` in a compilable handler"),
        ))
    }

    pub(super) fn unary(
        &mut self,
        unary: &syn::ExprUnary,
    ) -> Result<(String, Ty), Error> {
        let (text, ty) = self.expr(&unary.expr)?;
        match unary.op {
            UnOp::Not(_) => Ok((format!("(not {text})"), Ty::BOOL)),
            UnOp::Neg(_) => Ok((format!("(-{text})"), ty)),
            UnOp::Deref(_) => Ok((text, ty)),
            other => Err(Error::at(other.span(), "unsupported unary operator")),
        }
    }

    pub(super) fn binary(
        &mut self,
        binary: &ExprBinary,
    ) -> Result<(String, Ty), Error> {
        if let Some(operator) = compound_operator(&binary.op) {
            self.assign(&binary.left, &binary.right, operator)?;
            return Ok((String::new(), Ty::VOID));
        }
        if matches!(binary.op, BinOp::Eq(_) | BinOp::Ne(_)) {
            let negated = matches!(binary.op, BinOp::Ne(_));
            self.record_event(&binary.left, &binary.right, negated);
            self.record_event(&binary.right, &binary.left, negated);
        }
        let (left, left_ty) = self.expr(&binary.left)?;
        let (right, right_ty) = self.expr(&binary.right)?;
        if let Some(operator) = compare_operator(&binary.op) {
            if let Some(ordering) = path_ordering(&binary.op)
                && (left_ty.base == Base::Path || right_ty.base == Base::Path)
            {
                return Ok((
                    format!("({} {ordering} 0)", path_compare(&left, &right)),
                    Ty::BOOL,
                ));
            }
            return Ok((format!("({left} {operator} {right})"), Ty::BOOL));
        }
        let integer = left_ty.base == Base::Int && right_ty.base == Base::Int;
        match binary.op {
            BinOp::Add(_) => Ok((
                format!("({left} + {right})"),
                numeric(&left_ty, &right_ty),
            )),
            BinOp::Sub(_) => Ok((
                format!("({left} - {right})"),
                numeric(&left_ty, &right_ty),
            )),
            BinOp::Mul(_) => Ok((
                format!("({left} * {right})"),
                numeric(&left_ty, &right_ty),
            )),
            BinOp::Div(_) if integer => {
                Ok((format!("(math.modf({left} / {right}))"), Ty::INT))
            }
            BinOp::Div(_) => Ok((
                format!("({left} / {right})"),
                numeric(&left_ty, &right_ty),
            )),
            BinOp::Rem(_) if integer => {
                self.uses_irem = true;
                Ok((format!("irem({left}, {right})"), Ty::INT))
            }
            BinOp::Rem(_) => Ok((
                format!("({left} % {right})"),
                numeric(&left_ty, &right_ty),
            )),
            other => {
                Err(Error::at(other.span(), "unsupported binary operator"))
            }
        }
    }

    pub(super) fn field(
        &mut self,
        _expr: &Expr,
        field: &syn::ExprField,
    ) -> Result<(String, Ty), Error> {
        let member = match &field.member {
            Member::Named(name) => name.to_string(),
            Member::Unnamed(index) => {
                if index.index == 0
                    && let Expr::Call(call) = strip_reference(&field.base)
                    && call.args.len() == 1
                    && path_last_name(&call.func).is_some_and(|name| {
                        self.program.is_newtype(&self.module, &name)
                    })
                {
                    let args = call.args.iter().collect::<Vec<_>>();
                    let (value, value_ty) = self.expr(arg(&args, 0, call)?)?;
                    self.newtype_types.insert(value.clone(), value_ty);
                    return Ok((value, value_ty));
                }
                let (base, base_ty) = self.expr(&field.base)?;
                if index.index == 0
                    && (self.class_of_receiver(&field.base).is_some_and(
                        |class| self.program.is_newtype(&self.module, &class),
                    ) || self.newtype_types.contains_key(&base))
                {
                    let ty = path_ident(strip_reference(&field.base))
                        .and_then(|name| self.newtype_types.get(&name).copied())
                        .or_else(|| self.newtype_types.get(&base).copied())
                        .unwrap_or(base_ty);
                    self.newtype_types.insert(base.clone(), ty);
                    return Ok((base, ty));
                }
                let key = self
                    .tuple_expr_text
                    .get(&base)
                    .cloned()
                    .unwrap_or_else(|| base.clone());
                let ty = self
                    .tuple_expr_shapes
                    .get(&key)
                    .and_then(|shapes| shapes.get(index.index as usize))
                    .map(|shape| match shape {
                        TupleShape::Leaf(ty) => *ty,
                        TupleShape::Tuple(parts) => Ty {
                            null: parts.iter().any(TupleShape::any_null),
                            ..Ty::VALUE
                        },
                    })
                    .unwrap_or(Ty::VALUE);
                return Ok((format!("{base}[{}]", index.index + 1), ty));
            }
        };
        if self.is_ctx_expr(&field.base) {
            return match member.as_str() {
                "event" => Ok(match &self.bound_event {
                    Some(event) => (quote(event), Ty::STR),
                    None => {
                        self.uses_event = true;
                        ("event".to_owned(), Ty::STR)
                    }
                }),
                "now_ms" => Ok(("noctalia.nowMs()".to_owned(), Ty::INT)),
                "entry" => {
                    self.uses_entry = true;
                    Ok(("entry".to_owned(), Ty::STR))
                }
                "payload" => {
                    self.uses_payload = true;
                    self.payload_escapes = true;
                    Ok(("payload".to_owned(), Ty::VALUE))
                }
                "state" => {
                    self.uses_state = true;
                    Ok(("plugin_state".to_owned(), Ty::VALUE))
                }
                "data_dir" => Ok((
                    "(noctalia.pluginDataDir() or noctalia.pluginDir())"
                        .to_owned(),
                    Ty::PATH,
                )),
                "config" => {
                    if self.strict() {
                        self.uses_config_table = true;
                        return Ok(("CONFIG".to_owned(), Ty::VALUE));
                    }
                    Ok(("noctalia".to_owned(), Ty::VALUE))
                }
                other => Err(Error::at(
                    field.member.span(),
                    format!(
                        "context field `{other}` is not supported by the Luau compiler"
                    ),
                )),
            };
        }
        let (base, base_ty) = self.expr(&field.base)?;
        if base_ty.base == Base::Info {
            return Ok(match member.as_str() {
                "size" => (format!("{base}.size"), Ty::INT),
                "mtime" => (format!("{base}.mtime"), Ty::INT),
                "is_dir" => (format!("{base}.isDir"), Ty::BOOL),
                _ => (format!("{base}[{}]", quote(&member)), Ty::OPT_VALUE),
            });
        }
        let class = self.class_of_receiver(&field.base);
        let declared = class
            .as_deref()
            .and_then(|class| self.program.struct_def(&self.module, class))
            .and_then(|def| {
                def.fields
                    .iter()
                    .find(|(name, _)| name == &member)
                    .map(|(_, ty)| ty.clone())
            });
        let field_ty = class
            .as_deref()
            .and_then(|class| self.program.struct_def(&self.module, class))
            .and_then(|def| {
                def.fields
                    .iter()
                    .find(|(name, _)| name == &member)
                    .map(|(_, ty)| type_hint(ty))
            })
            .unwrap_or(Ty::OPT_VALUE);
        let text = if is_identifier(&member) {
            format!("{base}.{member}")
        } else {
            format!("{base}[{}]", quote(&member))
        };
        let mut field_ty = field_ty;
        if let Some(shapes) = self.tuple_expr_shapes.get(&text).cloned() {
            self.last_tuple_shape = Some(shapes);
        } else if let Some(Type::Tuple(items)) = declared.as_ref()
            && items.elems.iter().any(|item| {
                matches!(item, Type::Path(path)
                    if path.path.segments.last().is_some_and(|s| s.ident == "Value"))
            })
        {
            self.unknown_value_tuple = true;
        }
        if let Some(name) = path_ident(strip_reference(&field.base))
            && self
                .null_fields
                .get(&name)
                .is_some_and(|nulls| nulls.contains(&member))
        {
            field_ty.null = true;
        }
        Ok((text, field_ty))
    }

    pub(super) fn index(
        &mut self,
        index: &syn::ExprIndex,
    ) -> Result<(String, Ty), Error> {
        if let Expr::Range(range) = strip_reference(&index.index) {
            let (base, base_ty) = self.expr(&index.expr)?;
            if base_ty.base != Base::Str {
                return Err(Error::at(
                    range.span(),
                    "range indexes need a string receiver, and this one is not a string",
                ));
            }
            let start = match &range.start {
                Some(start) => {
                    let (text, _) = self.expr(strip_reference(start))?;
                    format!("({text}) + 1")
                }
                None => "1".to_owned(),
            };
            let stop = match &range.end {
                Some(end) => {
                    let (text, _) = self.expr(strip_reference(end))?;
                    text
                }
                None => format!("#{base}"),
            };
            return Ok((
                format!("string.sub({base}, {start}, {stop})"),
                Ty::STR,
            ));
        }
        let (key, key_ty) = self.expr(&index.index)?;
        if is_config_expr(&index.expr) {
            if self.strict() {
                strict_config_key(&index.index)?;
            }
            if self.strict() {
                self.uses_config_table = true;
                return Ok((format!("CONFIG[{key}]"), Ty::OPT_VALUE));
            }
            return Ok((
                self.coerced_string(format!("noctalia.getConfig({key})"), true),
                Ty::OPT_VALUE,
            ));
        }
        let (base, _) = self.expr(&index.expr)?;
        let base = if base.starts_with('{') {
            format!("({base})")
        } else {
            base
        };
        if key_ty.base == Base::Int {
            Ok((format!("{base}[({key}) + 1]"), Ty::OPT_VALUE))
        } else {
            Ok((format!("{base}[{key}]"), Ty::OPT_VALUE))
        }
    }

    fn cast(&mut self, cast: &syn::ExprCast) -> Result<(String, Ty), Error> {
        let (text, _) = self.expr(&cast.expr)?;
        Ok((text, type_hint(&cast.ty)))
    }

    fn try_expr(&mut self, expr: &Expr) -> Result<(String, Ty), Error> {
        let (text, ty) = self.expr(expr)?;
        let name = self.temp();
        if ty.result {
            let error = self.temp();
            self.line(&format!("local {name}, {error} = {text}"));
            if self.helper_result {
                self.line(&format!(
                    "if {error} ~= nil then return nil, {error} end"
                ));
            } else {
                self.line(&format!(
                    "if {error} ~= nil then error(tostring({error})) end"
                ));
            }
        } else {
            self.line(&format!("local {name} = {text}"));
            if ty.optional {
                let bare = if self.helper_pair {
                    "return nil, nil"
                } else if self.helper_result {
                    "return nil"
                } else {
                    "return"
                };
                self.line(&format!("if {name} == nil then {bare} end"));
            }
        }
        Ok((
            name,
            Ty {
                optional: false,
                result: false,
                ..ty
            },
        ))
    }

    pub(super) fn closure(
        &mut self,
        closure: &syn::ExprClosure,
    ) -> Result<(String, Ty), Error> {
        let saved_out = std::mem::take(&mut self.out);
        let saved_indent = self.indent;
        let saved_target = self.tail_target.clone();
        let saved_deferred = self.deferred;
        let saved_result = self.helper_result;
        let saved_pair = self.helper_pair;
        self.helper_result = false;
        self.helper_pair = false;
        self.deferred = self.deferred.saturating_add(1);
        self.indent = 0;
        self.tail_target = None;
        self.push_scope();
        self.local_statics.push(HashMap::new());
        let mut params = Vec::new();
        for input in &closure.inputs {
            let input = match input {
                Pat::Type(typed) => &*typed.pat,
                other => other,
            };
            match input {
                syn::Pat::Ident(pattern) => {
                    let name = pattern.ident.to_string();
                    self.push(&name, Ty::VALUE);
                    params.push(self.bind_name(&name));
                }
                syn::Pat::Wild(_) => params.push("_".to_owned()),
                other => {
                    self.local_statics.pop();
                    self.pop_scope();
                    self.out = saved_out;
                    self.indent = saved_indent;
                    self.tail_target = saved_target;
                    self.deferred = saved_deferred;
                    self.helper_result = saved_result;
                    self.helper_pair = saved_pair;
                    return Err(Error::at(
                        other.span(),
                        "closure parameters must be plain identifiers",
                    ));
                }
            }
        }
        self.out
            .push_str(&format!("function({})\n", params.join(", ")));
        self.indent = 1;
        let result = match &*closure.body {
            Expr::Block(block) => self.block_inner(&block.block, true),
            body => self.tail_expr(body),
        };
        self.indent = 0;
        self.line("end");
        let text = std::mem::replace(&mut self.out, saved_out);
        self.local_statics.pop();
        self.pop_scope();
        self.indent = saved_indent;
        self.tail_target = saved_target;
        self.deferred = saved_deferred;
        self.helper_result = saved_result;
        self.helper_pair = saved_pair;
        result?;
        Ok((text, Ty::VALUE))
    }

    /// A closure body whose only statement is a call to a unit helper stays a
    /// statement; anything else is an expression.
    fn value_or_statement(
        &mut self,
        expr: &Expr,
    ) -> Result<(String, Ty), Error> {
        if let Expr::Call(call) = expr
            && !is_ok_call(call)
            && !is_ops_push(self, call)
            && let Some(helper) = self.helper(call)
            && matches!(return_kind(&helper.function)?, Returns::Unit)
        {
            let bindings = self.bindings(&helper, call)?;
            self.inline_resolved(
                helper,
                Inline::Statement,
                call.span(),
                bindings,
            )?;
            return Ok((String::new(), Ty::VOID));
        }
        self.expr(expr)
    }
    fn emit_chain_parts(&mut self, parts: &[&Expr]) -> Result<(), Error> {
        let Some((part, rest)) = parts.split_first() else {
            self.line("return true");
            return Ok(());
        };
        let (condition, bindings) = match part {
            Expr::Let(item) => {
                let (condition, bindings, ty) =
                    self.let_condition_pair(item)?;
                for (name, _) in &bindings {
                    let binding = self.binding_ty(&item.pat, name, ty);
                    self.push(name, binding);
                }
                (condition, bindings)
            }
            other => (self.expr(other)?.0, Vec::new()),
        };
        self.line(&format!("if {condition} then"));
        self.indent = self.indent.saturating_add(1);
        for (name, text) in bindings {
            let lua = self.bind_name(&name);
            self.line(&format!("{lua} = {text}"));
        }
        self.emit_chain_parts(rest)?;
        self.indent = self.indent.saturating_sub(1);
        self.line("end");
        Ok(())
    }

    fn chain_condition(
        &mut self,
        binary: &ExprBinary,
    ) -> Result<(String, Ty), Error> {
        let mut parts = Vec::new();
        let owned = Expr::Binary(binary.clone());
        collect_and(&owned, &mut parts);
        for part in &parts {
            if let Expr::Let(item) = part {
                let mut names = Vec::new();
                pattern_binding_names(&item.pat, &mut names);
                for name in names {
                    if self.lookup(&name).is_some() {
                        return Err(Error::at(
                            binary.span(),
                            "let-chain bindings cannot shadow an outer local",
                        ));
                    }
                    let lua = self.bind_name(&name);
                    self.line(&format!("local {lua} = nil"));
                }
            }
        }
        let saved_out = std::mem::take(&mut self.out);
        let saved_indent = self.indent;
        self.indent = 0;
        self.emit_chain_parts(&parts)?;
        self.line("return false");
        let body = std::mem::replace(&mut self.out, saved_out);
        self.indent = saved_indent;
        Ok((format!("(function()\n{body}end)()"), Ty::BOOL))
    }

    fn receiver_is_chars(&self, expr: &Expr) -> bool {
        match strip_reference(expr) {
            Expr::Path(path) => path.path.get_ident().is_some_and(|ident| {
                self.is_chars(&self.local_name(&ident.to_string()))
            }),
            Expr::MethodCall(call) if call.method == "chars" => true,
            Expr::MethodCall(call) => {
                matches!(call.method.to_string().as_str(), "skip" | "take")
                    && self.receiver_is_chars(&call.receiver)
            }
            _ => false,
        }
    }

    fn bind_closure_pattern(
        &mut self,
        pattern: &Pat,
        value: &str,
        ty: Option<Ty>,
    ) -> Result<(), Error> {
        match pattern {
            Pat::Ident(pattern) => {
                let name = pattern.ident.to_string();
                let lua = self.bind_name(&name);
                self.line(&format!("local {lua} = {value}"));
                self.push(&name, ty.unwrap_or(Ty::VALUE));
                Ok(())
            }
            Pat::Wild(_) => Ok(()),
            Pat::Reference(reference) => {
                self.bind_closure_pattern(&reference.pat, value, ty)
            }
            Pat::Paren(paren) => {
                self.bind_closure_pattern(&paren.pat, value, ty)
            }
            Pat::Type(typed) => {
                self.bind_closure_pattern(&typed.pat, value, ty)
            }
            Pat::Tuple(tuple) => {
                for (index, element) in tuple.elems.iter().enumerate() {
                    let item = ty.map(|outer| match outer.element {
                        Some(base) if index == 0 => Ty::new(base),
                        Some(_) => Ty::VALUE,
                        None => outer,
                    });
                    let source = if value.starts_with('{') {
                        format!("({value})")
                    } else {
                        value.to_owned()
                    };
                    self.bind_closure_pattern(
                        element,
                        &format!("{source}[{}]", index + 1),
                        item,
                    )?;
                }
                Ok(())
            }
            other => Err(Error::at(
                other.span(),
                "closure parameters must be plain identifiers or tuples",
            )),
        }
    }

    pub(super) fn closure_value(
        &mut self,
        closure: &syn::ExprClosure,
        args: &[String],
    ) -> Result<(String, Ty), Error> {
        self.closure_value_typed(closure, args, &[], None)
    }

    pub(super) fn closure_value_typed(
        &mut self,
        closure: &syn::ExprClosure,
        args: &[String],
        types: &[Option<Ty>],
        pair_types: Option<(Ty, Ty)>,
    ) -> Result<(String, Ty), Error> {
        if closure.inputs.len() != args.len() {
            return Err(Error::at(
                closure.span(),
                format!(
                    "closure takes {} arguments but {} were passed",
                    closure.inputs.len(),
                    args.len()
                ),
            ));
        }
        self.push_scope();
        self.local_statics.push(HashMap::new());
        for ((input, value), ty) in closure
            .inputs
            .iter()
            .zip(args)
            .zip(types.iter().copied().chain(std::iter::repeat(None)))
        {
            let input = match input {
                Pat::Type(typed) => &*typed.pat,
                other => other,
            };
            let bound = if let Some((key_ty, value_ty)) = pair_types
                && closure.inputs.len() == 1
                && let Pat::Tuple(tuple) = input
                && tuple.elems.len() == 2
            {
                let source = if value.starts_with('{') {
                    format!("({value})")
                } else {
                    value.to_owned()
                };
                let first = self.bind_closure_pattern(
                    &tuple.elems[0],
                    &format!("{source}[1]"),
                    Some(key_ty),
                );
                let second = self.bind_closure_pattern(
                    &tuple.elems[1],
                    &format!("{source}[2]"),
                    Some(value_ty),
                );
                first.and(second)
            } else {
                self.bind_closure_pattern(input, value, ty)
            };
            if let Err(error) = bound {
                self.local_statics.pop();
                self.pop_scope();
                return Err(error);
            }
        }
        let result = match &*closure.body {
            Expr::Block(block) => match block.block.stmts.split_last() {
                Some((last, rest)) => {
                    for stmt in rest {
                        self.stmt(stmt)?;
                    }
                    match last {
                        Stmt::Expr(expr, _) => {
                            if self.in_statement
                                && !rest.is_empty()
                                && let Expr::Call(call) = expr
                                && let Some(helper) = self.helper(call)
                                && !helper.name.ends_with("retain")
                                && matches!(
                                    return_kind(&helper.function)?,
                                    Returns::Value(_)
                                )
                            {
                                let bindings = self.bindings(&helper, call)?;
                                self.inline_resolved(
                                    helper,
                                    Inline::Statement,
                                    call.span(),
                                    bindings,
                                )?;
                                Ok((String::new(), Ty::VOID))
                            } else {
                                self.value_or_statement(expr)
                            }
                        }
                        _ => Ok(("nil".to_owned(), Ty::VALUE)),
                    }
                }
                None => Ok(("nil".to_owned(), Ty::NULL)),
            },
            body => self.value_or_statement(body),
        };
        self.local_statics.pop();
        self.pop_scope();
        result
    }
}

pub(super) fn is_config_expr(expr: &Expr) -> bool {
    let Expr::Field(field) = strip_reference(expr) else {
        return false;
    };
    matches!(&field.member, Member::Named(name) if name == "config")
        && matches!(&*field.base, Expr::Path(_))
}

pub(super) fn default_value(
    emitter: &Emitter<'_>,
    ty: &Type,
    span: proc_macro2::Span,
) -> Result<String, Error> {
    let ty = match ty {
        Type::Reference(reference) => &*reference.elem,
        other => other,
    };
    let name = type_text(ty);
    if name == "Option" {
        return Ok("nil".to_owned());
    }
    let literal = match name.as_str() {
        "String" | "str" => Some("\"\""),
        "bool" => Some("false"),
        "i64" | "u64" | "usize" | "u32" | "i32" | "f64" | "f32" => Some("0"),
        "Value" => Some("nil"),
        "Vec" | "VecDeque" | "HashMap" | "BTreeMap" | "HashSet"
        | "BTreeSet" | "Map" | "IndexMap" | "IndexSet" => Some("{}"),
        _ => None,
    };
    if let Some(literal) = literal {
        return Ok(literal.to_owned());
    }
    let def = emitter
        .program
        .struct_def(&emitter.module, &name)
        .ok_or_else(|| {
            Error::at(
                span,
                format!("type `{name}` has no default in a compilable handler"),
            )
        })?;
    if let Some(config) = &def.config {
        let fields = config
            .iter()
            .map(|field| {
                format!("{} = {}", lua_key(&field.name), field.default.lua())
            })
            .collect::<Vec<_>>();
        return Ok(format!("{{ {} }}", fields.join(", ")));
    }
    if !def.derives_default {
        return Err(Error::at(
            span,
            format!("type `{name}` has no derived Default implementation"),
        ));
    }
    let mut fields = Vec::new();
    for (field, field_ty) in &def.fields {
        fields.push(format!(
            "{} = {}",
            lua_key(field),
            default_value(emitter, field_ty, span)?
        ));
    }
    Ok(format!("{{ {} }}", fields.join(", ")))
}

pub(super) fn path_ident(expr: &Expr) -> Option<String> {
    let Expr::Path(path) = expr else {
        return None;
    };
    path.path.get_ident().map(ToString::to_string)
}

fn compare_operator(operator: &BinOp) -> Option<&'static str> {
    match operator {
        BinOp::Eq(_) => Some("=="),
        BinOp::Ne(_) => Some("~="),
        BinOp::Lt(_) => Some("<"),
        BinOp::Le(_) => Some("<="),
        BinOp::Gt(_) => Some(">"),
        BinOp::Ge(_) => Some(">="),
        BinOp::And(_) => Some("and"),
        BinOp::Or(_) => Some("or"),
        _ => None,
    }
}

/// `Ord for Path` compares components, so `<` on two paths is not a byte
/// comparison.
fn path_compare(left: &str, right: &str) -> String {
    format!(
        "(function() local __left, __right = {left}, {right}; __left = string.gsub(__left, \"//+\", \"/\"); __right = string.gsub(__right, \"//+\", \"/\"); while #__left > 1 and string.sub(__left, -1) == \"/\" do __left = string.sub(__left, 1, -2) end; while #__right > 1 and string.sub(__right, -1) == \"/\" do __right = string.sub(__right, 1, -2) end; local __left_root = string.sub(__left, 1, 1) == \"/\"; local __right_root = string.sub(__right, 1, 1) == \"/\"; if __left_root ~= __right_root then return __left_root and -1 or 1 end; local __lefts, __rights = {{}}, {{}}; for __part in string.gmatch(__left, \"[^/]+\") do __lefts[#__lefts + 1] = __part end; for __part in string.gmatch(__right, \"[^/]+\") do __rights[#__rights + 1] = __part end; local __index = 1; while true do local __a, __b = __lefts[__index], __rights[__index]; if __a == nil and __b == nil then return 0 end; if __a == nil then return -1 end; if __b == nil then return 1 end; if __a ~= __b then if __a < __b then return -1 end; return 1 end; __index = __index + 1 end end)()"
    )
}

/// `Ord for Path` compares components, so ordering on two paths is not a byte
/// comparison. Equality stays the string equality of the same bytes.
fn path_ordering(operator: &BinOp) -> Option<&'static str> {
    match operator {
        BinOp::Lt(_) => Some("<"),
        BinOp::Le(_) => Some("<="),
        BinOp::Gt(_) => Some(">"),
        BinOp::Ge(_) => Some(">="),
        _ => None,
    }
}

fn compound_operator(operator: &BinOp) -> Option<&'static str> {
    match operator {
        BinOp::AddAssign(_) => Some("+="),
        BinOp::SubAssign(_) => Some("-="),
        BinOp::MulAssign(_) => Some("*="),
        _ => None,
    }
}

fn numeric(left: &Ty, right: &Ty) -> Ty {
    if left.base == Base::Float || right.base == Base::Float {
        Ty::FLOAT
    } else {
        Ty::INT
    }
}

pub(super) fn wider(left: Ty, right: Ty) -> Ty {
    let null = left.null || right.null;
    let mut ty = if left.base == right.base {
        left
    } else if left.is_numeric() && right.is_numeric() {
        Ty::FLOAT
    } else {
        Ty::VALUE
    };
    ty.null = null;
    ty.absent = left.absent && right.absent;
    ty.optional = left.optional || right.optional;
    ty
}

pub(super) fn path_last_name(expr: &Expr) -> Option<String> {
    let Expr::Path(path) = expr else {
        return None;
    };
    let segments = path
        .path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>();
    if segments.is_empty() {
        None
    } else {
        Some(segments.join("::"))
    }
}

/// `Default::default` passed as a callback, so the type comes from the caller.
pub(super) fn is_default_path(expr: &Expr) -> bool {
    matches!(expr, Expr::Path(_))
        && path_last_name(expr).as_deref() == Some("Default::default")
}

/// `Default::default()` called without a type the emitter can read.
pub(super) fn is_default_call(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Call(call)
            if call.args.is_empty()
                && path_last_name(&call.func).as_deref()
                    == Some("Default::default")
    )
}

pub(super) fn bare_default_error(span: proc_macro2::Span) -> Error {
    Error::at(
        span,
        "`Default::default()` needs a known type annotation to pick its literal",
    )
}

pub(super) fn is_ok_call(call: &ExprCall) -> bool {
    path_last_name(&call.func).as_deref() == Some("Ok")
}

pub(super) fn is_err_call(call: &ExprCall) -> bool {
    path_last_name(&call.func).as_deref() == Some("Err")
}

pub(super) fn is_vec_new(call: &ExprCall) -> bool {
    path_last_name(&call.func).as_deref() == Some("Vec::new")
}

pub(super) fn multi_return(expr: &Expr) -> bool {
    match expr {
        Expr::MethodCall(call) => {
            call.method.to_string().ends_with("_result")
                || matches!(
                    call.method.to_string().as_str(),
                    "query_pair" | "query_response"
                )
        }
        Expr::Call(call) => path_last_name(&call.func)
            .is_some_and(|name| name.ends_with("_result")),
        _ => false,
    }
}

pub(super) fn arg<'a>(
    args: &[&'a Expr],
    index: usize,
    call: &impl Spanned,
) -> Result<&'a Expr, Error> {
    args.get(index).copied().ok_or_else(|| {
        Error::at(call.span(), format!("missing argument {index}"))
    })
}

pub(super) fn expr_string(expr: &Expr) -> Result<String, Error> {
    let Expr::Lit(ExprLit {
        lit: Lit::Str(value),
        ..
    }) = expr
    else {
        return Err(Error::at(expr.span(), "expected a string literal"));
    };
    Ok(value.value())
}

pub(super) fn unquote(value: &str) -> String {
    value.trim_matches('"').to_owned()
}

pub(super) fn lua_key(key: &str) -> String {
    if is_identifier(key) {
        key.to_owned()
    } else {
        format!("[{}]", quote(key))
    }
}

pub(super) fn strip_reference(expr: &Expr) -> &Expr {
    match expr {
        Expr::Reference(reference) => strip_reference(&reference.expr),
        Expr::Paren(paren) => strip_reference(&paren.expr),
        Expr::Group(group) => strip_reference(&group.expr),
        other => other,
    }
}

fn match_returns_string(item: &syn::ExprMatch) -> bool {
    !item.arms.is_empty()
        && item.arms.iter().all(|arm| {
            let expr = match &*arm.body {
                Expr::Block(block) => {
                    block.block.stmts.last().and_then(|stmt| {
                        if let Stmt::Expr(expr, _) = stmt {
                            Some(expr)
                        } else {
                            None
                        }
                    })
                }
                other => Some(other),
            };
            expr.is_some_and(|expr| match strip_reference(expr) {
                Expr::Lit(ExprLit {
                    lit: Lit::Str(_), ..
                }) => true,
                Expr::MethodCall(call) => {
                    matches!(
                        call.method.to_string().as_str(),
                        "to_owned" | "to_string"
                    )
                }
                Expr::Macro(mac) => macro_name(mac) == "format",
                Expr::Call(call) => {
                    path_last_name(&call.func).as_deref() == Some("String::new")
                }
                _ => false,
            })
        })
}

fn collect_and<'a>(expr: &'a Expr, parts: &mut Vec<&'a Expr>) {
    if let Expr::Binary(binary) = expr
        && matches!(binary.op, BinOp::And(_))
    {
        collect_and(&binary.left, parts);
        collect_and(&binary.right, parts);
    } else {
        parts.push(expr);
    }
}

fn and_let_count(expr: &Expr) -> usize {
    let mut parts = Vec::new();
    collect_and(expr, &mut parts);
    parts
        .iter()
        .filter(|part| matches!(part, Expr::Let(_)))
        .count()
}

fn pattern_binding_names(pattern: &Pat, names: &mut Vec<String>) {
    match pattern {
        Pat::Ident(ident) => names.push(ident.ident.to_string()),
        Pat::Tuple(tuple) => {
            for element in &tuple.elems {
                pattern_binding_names(element, names);
            }
        }
        Pat::TupleStruct(item) => {
            for element in &item.elems {
                pattern_binding_names(element, names);
            }
        }
        Pat::Paren(paren) => pattern_binding_names(&paren.pat, names),
        Pat::Type(typed) => pattern_binding_names(&typed.pat, names),
        Pat::Reference(reference) => {
            pattern_binding_names(&reference.pat, names)
        }
        Pat::Or(or) => {
            if let Some(first) = or.cases.first() {
                pattern_binding_names(first, names);
            }
        }
        Pat::Struct(item) => {
            for field in &item.fields {
                pattern_binding_names(&field.pat, names);
            }
        }
        _ => {}
    }
}
