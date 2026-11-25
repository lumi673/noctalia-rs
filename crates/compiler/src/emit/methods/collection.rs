use super::*;

impl<'a> Emitter<'a> {
    pub(super) fn collection_method(
        &mut self,
        call: &ExprMethodCall,
        method: &str,
        args: &[&Expr],
        receiver: &str,
        ty: Ty,
    ) -> Result<OptionalValue, Error> {
        let value: Result<(String, Ty), Error> = match method {
            "to_string" => Ok((format!("tostring({receiver})"), Ty::STR)),
            "clone" | "cloned" | "to_owned" | "into_owned"
                if matches!(
                    ty.base,
                    Base::Value | Base::Table | Base::List
                ) =>
            {
                self.uses_deep_clone = true;
                Ok((format!("deep_clone({receiver})"), ty))
            }
            "into_owned" | "to_owned" | "clone" | "as_ref" | "as_deref"
            | "cloned" | "into_inner" | "into" | "as_mut" | "as_slice"
            | "as_bytes" | "into_bytes" | "copied" | "peekable"
            | "iter_mut" => Ok((receiver.to_owned(), ty)),
            "first" => Ok((format!("({receiver}[1])"), Ty::OPT_VALUE)),
            "pop" => Ok((format!("table.remove({receiver})"), Ty::OPT_VALUE)),
            "push_back" => {
                let item = arg(args, 0, call)?;
                let (text, value_ty) = self.expr(item)?;
                reject_null(&text, value_ty, item.span(), "push_back")?;
                Ok((format!("table.insert({receiver}, {text})"), Ty::VOID))
            }
            "push" if args.len() == 1 && ty.base != Base::Ops => {
                let item = arg(args, 0, call)?;
                let (text, value_ty) = self.expr(item)?;
                reject_null(&text, value_ty, item.span(), "push")?;
                Ok((format!("table.insert({receiver}, {text})"), Ty::VOID))
            }
            "flatten" if ty.optional || ty.base == Base::List => {
                if ty.optional && ty.base != Base::List {
                    return Err(Error::at(
                        call.method.span(),
                        "flatten() needs an optional list or a list of lists",
                    ));
                }
                if ty.optional {
                    Ok((format!("({receiver} or {{}})"), Ty::LIST))
                } else {
                    Ok((
                        format!(
                            "(function() local __out = {{}}; local __lists = {receiver}; for __i = 1, #__lists do local __inner = __lists[__i]; if type(__inner) == \"table\" then for __j = 1, #__inner do __out[#__out + 1] = __inner[__j] end end end; __out.__next = 1; return __out end)()"
                        ),
                        Ty::LIST,
                    ))
                }
            }
            "skip" if ty.base == Base::List && !ty.optional => {
                let count = self.expr(arg(args, 0, call)?)?.0;
                Ok((
                    format!(
                        "(function() local __out = {{}}; local __n = math.max(0, {count}); for __i = __n + 1, #{receiver} do __out[#__out + 1] = {receiver}[__i] end; __out.__next = 1; return __out end)()"
                    ),
                    Ty::LIST,
                ))
            }
            "take" if ty.base == Base::List && !ty.optional => {
                let count = self.expr(arg(args, 0, call)?)?.0;
                Ok((
                    format!(
                        "(function() local __out = {{}}; local __n = math.min(math.max(0, {count}), #{receiver}); for __i = 1, __n do __out[#__out + 1] = {receiver}[__i] end; __out.__next = 1; return __out end)()"
                    ),
                    Ty::LIST,
                ))
            }
            "extend_from_slice" => {
                let (value, _) = self.expr(arg(args, 0, call)?)?;
                let item = self.temp();
                self.line(&format!(
                    "for _, {item} in ipairs({value}) do table.insert({receiver}, {item}) end"
                ));
                Ok((String::new(), Ty::VOID))
            }
            "truncate" if ty.base == Base::List && !ty.optional => {
                let length = self.expr(arg(args, 0, call)?)?.0;
                self.line(&format!(
                    "for __i = #{receiver}, ({length}) + 1, -1 do table.remove({receiver}, __i) end"
                ));
                Ok((String::new(), Ty::VOID))
            }
            "retain" => {
                let closure = arg(args, 0, call)?;
                let arity = match closure {
                    Expr::Closure(closure) => closure.inputs.len(),
                    _ => 1,
                };
                if ty.base == Base::Table {
                    if arity != 2 {
                        return Err(Error::at(
                            call.method.span(),
                            "retain() on a map needs a closure with key and value",
                        ));
                    }
                    let key = self.temp();
                    let value = self.temp();
                    self.line(&format!(
                        "for {key}, {value} in pairs({receiver}) do"
                    ));
                    self.indent = self.indent.saturating_add(1);
                    let (text, _) = self
                        .invoke_arg(closure, &[key.clone(), value.clone()])?;
                    self.line(&format!(
                        "if not ({text}) then {receiver}[{key}] = nil end"
                    ));
                    self.indent = self.indent.saturating_sub(1);
                    self.line("end");
                } else if ty.base == Base::List {
                    if arity != 1 {
                        return Err(Error::at(
                            call.method.span(),
                            "retain() on a list needs a closure with one argument",
                        ));
                    }
                    let index = self.temp();
                    self.line(&format!("for {index} = #{receiver}, 1, -1 do"));
                    self.indent = self.indent.saturating_add(1);
                    let item = self.temp();
                    self.line(&format!("local {item} = {receiver}[{index}]"));
                    let (text, _) =
                        self.invoke_arg(closure, std::slice::from_ref(&item))?;
                    self.line(&format!(
                        "if not ({text}) then table.remove({receiver}, {index}) end"
                    ));
                    self.indent = self.indent.saturating_sub(1);
                    self.line("end");
                } else {
                    return Err(Error::at(
                        call.method.span(),
                        "retain() needs a list or map receiver",
                    ));
                }
                Ok((String::new(), Ty::VOID))
            }
            "sort_by" | "sort_unstable_by" | "sort_by_key" => Err(Error::at(
                call.method.span(),
                format!(
                    "`{method}` needs a Rust comparator and is not supported by the Luau compiler"
                ),
            )),
            "then_some" => {
                let value = self.expr(arg(args, 0, call)?)?.0;
                let stored = self.temp();
                self.line(&format!("local {stored} = {value}"));
                Ok((
                    format!(
                        "(function() if {receiver} then return {stored} end end)()"
                    ),
                    Ty::OPT_VALUE,
                ))
            }
            "is_none_or" => {
                let closure = arg(args, 0, call)?;
                let value = self.temp();
                let result = self.temp();
                self.line(&format!("local {value} = {receiver}"));
                self.line(&format!("local {result} = true"));
                self.line(&format!("if {value} ~= nil then"));
                self.indent = self.indent.saturating_add(1);
                let (text, _) =
                    self.invoke_arg(closure, std::slice::from_ref(&value))?;
                self.line(&format!("{result} = {text}"));
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                Ok((result, Ty::BOOL))
            }
            "or" if ty.optional => {
                let other = self.expr(arg(args, 0, call)?)?.0;
                let value = self.temp();
                self.line(&format!("local {value} = {receiver}"));
                Ok((
                    format!(
                        "(function() if {value} ~= nil then return {value} end return {other} end)()"
                    ),
                    ty,
                ))
            }
            "is_some" => Ok((format!("({receiver} ~= nil)"), Ty::BOOL)),
            "is_none" => Ok((format!("({receiver} == nil)"), Ty::BOOL)),
            "is_ok" => Ok((format!("({receiver} ~= nil)"), Ty::BOOL)),
            "is_err" => Ok((format!("({receiver} == nil)"), Ty::BOOL)),
            "join" if args.len() == 1 => {
                let part = self.expr(arg(args, 0, call)?)?.0;
                if ty.base == Base::List {
                    return Ok(Some((
                        format!("table.concat({receiver}, {part})"),
                        Ty::STR,
                    )));
                }
                if ty.base != Base::Str {
                    self.uses_join = true;
                    return Ok(Some((
                        format!("join({receiver}, {part})"),
                        Ty::STR,
                    )));
                }
                let base = self.temp();
                self.line(&format!("local {base} = {receiver}"));
                Ok((
                    format!(
                        "({base} .. ((string.sub({base}, -1) == \"/\") and \"\" or \"/\") .. {part})"
                    ),
                    Ty::STR,
                ))
            }
            "values" if matches!(ty.base, Base::Value | Base::Table) => {
                let values = self.temp();
                self.line(&format!(
                    "local {values} = (function() local __keys = {{}} for __key in pairs({receiver}) do __keys[#__keys + 1] = __key end table.sort(__keys, function(a, b) return tostring(a) < tostring(b) end) local __out = {{}} for __index, __key in ipairs(__keys) do __out[__index] = {receiver}[__key] end return __out end)()"
                ));
                Ok((values, Ty::LIST))
            }
            "iter" | "into_iter" | "values" => Ok((receiver.to_owned(), ty)),
            "next" => {
                let value = self.temp();
                let meta = self.temp();
                let cursor = self.temp();
                self.line(&format!("local {meta} = getmetatable({receiver})"));
                self.line(&format!(
                    "local {cursor} = ((type({meta}) == \"table\" and {meta}.{CURSOR_MARKER}) or {receiver}.__next or 0) + 1"
                ));
                self.line(&format!("local {value} = {receiver}[{cursor}]"));
                self.line(&format!("if {value} ~= nil then"));
                self.indent = self.indent.saturating_add(1);
                self.line(&format!(
                    "if type({meta}) == \"table\" and {meta}.{CURSOR_MARKER} ~= nil then"
                ));
                self.indent = self.indent.saturating_add(1);
                self.line(&format!("{meta}.{CURSOR_MARKER} = {cursor}"));
                self.indent = self.indent.saturating_sub(1);
                self.line(&format!("elseif {receiver}.__next ~= nil then"));
                self.indent = self.indent.saturating_add(1);
                self.line(&format!("{receiver}.__next = {cursor}"));
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                let element = ty.element.unwrap_or(Base::Value);
                Ok((value, Ty::opt(element)))
            }
            "as_os_str" | "to_str" | "to_vec" => {
                Ok((receiver.to_owned(), Ty::VALUE))
            }
            "clear" => {
                self.line(&format!("table.clear({receiver})"));
                Ok((String::new(), Ty::VOID))
            }
            "insert" if args.len() == 1 => {
                let value = self.expr(arg(args, 0, call)?)?.0;
                self.line(&format!("{receiver}[{value}] = true"));
                Ok((String::new(), Ty::VOID))
            }
            "insert" if args.len() == 2 => {
                let key = self.expr(arg(args, 0, call)?)?.0;
                let item = arg(args, 1, call)?;
                let (value, value_ty) = self.expr(item)?;
                reject_null(&value, value_ty, item.span(), "insert")?;
                if ty.base != Base::Table
                    && key.chars().all(|character| character.is_ascii_digit())
                {
                    self.line(&format!(
                        "table.insert({receiver}, ({key}) + 1, {value})"
                    ));
                    return Ok(Some((String::new(), Ty::VOID)));
                }
                if self.in_statement {
                    self.line(&format!("{receiver}[{key}] = {value}"));
                    return Ok(Some((String::new(), Ty::VOID)));
                }
                let previous = self.temp();
                self.line(&format!("local {previous} = {receiver}[{key}]"));
                self.line(&format!("{receiver}[{key}] = {value}"));
                Ok((previous, Ty::OPT_VALUE))
            }
            "remove" if args.len() == 1 => {
                let (key, key_ty) = self.expr(arg(args, 0, call)?)?;
                let removed = self.temp();
                if ty.base == Base::Table {
                    self.line(&format!("local {removed} = {receiver}[{key}]"));
                    self.line(&format!("{receiver}[{key}] = nil"));
                    return Ok(Some((removed, Ty::OPT_VALUE)));
                }
                if ty.base == Base::List && key_ty.base == Base::Int {
                    self.line(&format!(
                        "local {removed} = table.remove({receiver}, ({key}) + 1)"
                    ));
                    return Ok(Some((removed, Ty::VALUE)));
                }
                if key_ty.base != Base::Int {
                    self.line(&format!("local {removed} = {receiver}[{key}]"));
                    self.line(&format!("{receiver}[{key}] = nil"));
                    return Ok(Some((removed, Ty::OPT_VALUE)));
                }
                // A JSON object key is a string, so an integer key on a value
                // whose container kind is not known indexes an array.
                self.line(&format!(
                    "local {removed} = table.remove({receiver}, ({key}) + 1)"
                ));
                Ok((removed, Ty::VALUE))
            }
            "expect" => {
                let (message, _) = self.expr(arg(args, 0, call)?)?;
                let value = self.temp();
                if ty.result {
                    let error = self.temp();
                    self.line(&format!("local {value}, {error} = {receiver}"));
                    self.line(&format!(
                        "if {error} ~= nil then error(tostring({message})) end"
                    ));
                } else {
                    self.line(&format!("local {value} = {receiver}"));
                    self.line(&format!(
                        "if {value} == nil then error(tostring({message})) end"
                    ));
                }
                Ok((value, Ty::new(ty.base)))
            }
            "parse" => {
                let ty = turbofish_type(call.turbofish.as_ref());
                Ok((receiver.to_owned(), ty))
            }
            "ok" if ty.result => {
                let value = self.temp();
                let error = self.temp();
                self.line(&format!("local {value}, {error} = {receiver}"));
                Ok((value, Ty::opt(ty.base)))
            }
            "ok" => Ok((
                receiver.to_owned(),
                Ty {
                    optional: true,
                    ..ty
                },
            )),
            "get_or_insert" | "get_or_insert_with" => {
                let value = arg(args, 0, call)?;
                let (text, value_ty) = if method == "get_or_insert" {
                    self.expr(value)?
                } else {
                    self.invoke_arg(value, &[])?
                };
                self.line(&format!("if {receiver} == nil then"));
                self.indent = self.indent.saturating_add(1);
                if text.is_empty() || value_ty.base == Base::Void {
                    self.line(&format!("{receiver} = true"));
                } else {
                    self.line(&format!("{receiver} = {text}"));
                }
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                Ok((receiver.to_owned(), value_ty))
            }
            "unwrap" if ty.result => {
                let value = self.temp();
                let error = self.temp();
                self.line(&format!("local {value}, {error} = {receiver}"));
                self.line(&format!(
                    "if {error} ~= nil then error(tostring({error})) end"
                ));
                Ok((value, Ty::new(ty.base)))
            }
            "unwrap" => {
                self.uses_unwrap = true;
                Ok((format!("unwrap({receiver})"), Ty::new(ty.base)))
            }
            "unwrap_or" => {
                let (default, default_ty) = self.expr(arg(args, 0, call)?)?;
                if !ty.result
                    && (ty.base == Base::Bool || default_ty.base == Base::Bool)
                {
                    self.uses_bool_or = true;
                    return Ok(Some((
                        format!("bool_or({receiver}, {default})"),
                        Ty::BOOL,
                    )));
                }
                if ty.result {
                    let value = self.temp();
                    let error = self.temp();
                    let result = self.temp();
                    self.line(&format!("local {value}, {error} = {receiver}"));
                    self.line(&format!("local {result} = {value}"));
                    self.line(&format!(
                        "if {error} ~= nil then {result} = {default} end"
                    ));
                    return Ok(Some((
                        result,
                        Ty::new(wider(ty, default_ty).base),
                    )));
                }
                let value = self.temp();
                let result = self.temp();
                self.line(&format!("local {value} = {receiver}"));
                self.line(&format!("local {result} = {value}"));
                self.line(&format!(
                    "if {value} == nil then {result} = {default} end"
                ));
                Ok((result, Ty::new(wider(ty, default_ty).base)))
            }
            "unwrap_or_default" => {
                let default = if ty.base == Base::Value {
                    "nil".to_owned()
                } else {
                    ty.default_literal().ok_or_else(|| {
                        Error::at(
                            call.span(),
                            "this type has no default in a compilable handler",
                        )
                    })?.to_owned()
                };
                if ty.result {
                    let value = self.temp();
                    let error = self.temp();
                    let result = self.temp();
                    self.line(&format!("local {value}, {error} = {receiver}"));
                    self.line(&format!("local {result} = {value}"));
                    self.line(&format!(
                        "if {error} ~= nil then {result} = {default} end"
                    ));
                    return Ok(Some((result, Ty::new(ty.base))));
                }
                let value = self.temp();
                let result = self.temp();
                self.line(&format!("local {value} = {receiver}"));
                self.line(&format!("local {result} = {value}"));
                self.line(&format!(
                    "if {value} == nil then {result} = {default} end"
                ));
                Ok((result, Ty::new(ty.base)))
            }
            "len" => Ok((format!("#{receiver}"), Ty::INT)),
            "is_empty" if ty.base == Base::Table => {
                self.uses_count_keys = true;
                Ok((format!("(count_keys({receiver}) == 0)"), Ty::BOOL))
            }
            "is_empty" if ty.base == Base::Value => {
                self.uses_count_keys = true;
                Ok((
                    format!(
                        "((type({receiver}) == \"table\" and count_keys({receiver}) == 0) or (type({receiver}) == \"string\" and #{receiver} == 0))"
                    ),
                    Ty::BOOL,
                ))
            }
            "is_empty" => Ok((format!("(#{receiver} == 0)"), Ty::BOOL)),
            "get" | "get_mut" => {
                let (key, key_ty) = self.expr(arg(args, 0, call)?)?;
                if ty.base != Base::Table && key_ty.base == Base::Int {
                    Ok((format!("{receiver}[({key}) + 1]"), Ty::OPT_VALUE))
                } else {
                    Ok((format!("{receiver}[{key}]"), Ty::OPT_VALUE))
                }
            }
            "push" if ty.base == Base::Ops => {
                let value = arg(args, 0, call)?;
                if is_op_expr(value) {
                    self.op_expr(value)?;
                } else {
                    let (text, value_ty) = self.expr(value)?;
                    reject_null(&text, value_ty, value.span(), "push")?;
                    self.line(&format!("table.insert({receiver}, {text})"));
                }
                Ok((String::new(), Ty::VOID))
            }
            "extend" => {
                let (value, _) = self.expr(arg(args, 0, call)?)?;
                let item = self.temp();
                self.line(&format!(
                    "for _, {item} in ipairs({value}) do {receiver}[#({receiver}) + 1] = {item} end"
                ));
                Ok((String::new(), Ty::VOID))
            }
            _ => return Ok(None),
        };
        Ok(Some(value?))
    }
}
