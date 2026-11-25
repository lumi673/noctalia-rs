use super::*;

impl<'a> Emitter<'a> {
    fn static_receiver(
        &mut self,
        expr: &Expr,
    ) -> Result<OptionalStatic, Error> {
        let Expr::Path(path) = strip_reference(expr) else {
            return Ok(None);
        };
        let Some(segments) = path_segments(&path.path) else {
            return Ok(None);
        };
        let Some(name) = segments.last() else {
            return Ok(None);
        };
        if let Some((lua, kind)) = self
            .local_statics
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
        {
            return Ok(Some((lua.clone(), *kind)));
        }
        if let Some(kind) = self.containers.get(name) {
            return Ok(Some((name.clone(), *kind)));
        }
        let Some((module, _)) =
            self.program.resolve_static(&self.module, &segments)
        else {
            return Ok(None);
        };
        let def = self.register_static(&module, name)?;
        Ok(Some((def.name, def.kind)))
    }

    fn static_method(
        &mut self,
        lua: &str,
        kind: StaticKind,
        call: &ExprMethodCall,
        args: &[&Expr],
    ) -> Result<(String, Ty), Error> {
        match call.method.to_string().as_str() {
            "call_once" => {
                let closure = arg(args, 0, call)?;
                self.line(&format!("if not {lua} then"));
                self.indent = self.indent.saturating_add(1);
                self.line(&format!("{lua} = true"));
                let (text, _) = self.invoke_arg(closure, &[])?;
                if !text.is_empty() {
                    self.line(&text);
                }
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                Ok((String::new(), Ty::VOID))
            }
            "get_or_init" | "get_or_try_init" => {
                let closure = arg(args, 0, call)?;
                self.line(&format!("if {lua} == nil then"));
                self.indent = self.indent.saturating_add(1);
                let (text, ty) = self.invoke_arg(closure, &[])?;
                if text.is_empty() || ty.base == Base::Void {
                    self.line(&format!("{lua} = true"));
                } else {
                    self.line(&format!("{lua} = {text}"));
                }
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                Ok((
                    lua.to_owned(),
                    Ty {
                        result: false,
                        ..ty
                    },
                ))
            }
            "set" => {
                let value = arg(args, 0, call)?;
                let (text, _) = self.expr(value)?;
                let previous = self.temp();
                self.line(&format!("local {previous} = ({lua} ~= nil)"));
                self.line(&format!(
                    "if not {previous} then {lua} = {text} end"
                ));
                if self.in_statement {
                    Ok((String::new(), Ty::VOID))
                } else {
                    Ok((previous, Ty::BOOL))
                }
            }
            "take" => {
                self.line(&format!("local __taken = {lua}"));
                self.line(&format!("{lua} = nil"));
                Ok(("__taken".to_owned(), Ty::VALUE))
            }
            "lock" | "read" | "write" => {
                Ok((lua.to_owned(), Ty::result(Base::Value)))
            }
            "get" | "get_mut" | "load" | "deref" | "borrow" | "borrow_mut" => {
                Ok((lua.to_owned(), Ty::VALUE))
            }
            "store" | "swap" => {
                let value = arg(args, 0, call)?;
                let (text, _) = self.expr(value)?;
                let previous = self.temp();
                self.line(&format!("local {previous} = {lua}"));
                self.line(&format!("{lua} = {text}"));
                Ok((previous, Ty::VALUE))
            }
            "fetch_add" | "fetch_sub" => {
                let value = arg(args, 0, call)?;
                let (text, _) = self.expr(value)?;
                let previous = self.temp();
                self.line(&format!("local {previous} = {lua}"));
                let operator =
                    if call.method == "fetch_add" { "+" } else { "-" };
                self.line(&format!("{lua} = {previous} {operator} {text}"));
                Ok((previous, Ty::INT))
            }
            "is_completed" | "is_finished" => {
                Ok((format!("({lua} ~= nil)"), Ty::BOOL))
            }
            _ => {
                let _ = kind;
                Err(Error::at(
                    call.method.span(),
                    format!(
                        "method `{}` on a static is not supported by the Luau compiler",
                        call.method
                    ),
                ))
            }
        }
    }

    pub(super) fn invoke_arg(
        &mut self,
        arg: &Expr,
        values: &[String],
    ) -> Result<(String, Ty), Error> {
        if let Expr::Closure(closure) = arg {
            return self.closure_value(closure, values);
        }
        if let Some((text, ty)) = self.function_reference(arg, values)? {
            return Ok((text, ty));
        }
        let (text, ty) = self.expr(arg)?;
        Ok((format!("({text})({})", values.join(", ")), ty))
    }

    fn combinator(
        &mut self,
        call: &ExprMethodCall,
        method: &str,
        args: &[&Expr],
        receiver: &str,
        ty: Ty,
    ) -> Result<OptionalValue, Error> {
        match method {
            "lock" | "read" | "write" => {
                return Ok(Some((receiver.to_owned(), Ty::result(ty.base))));
            }
            "map_err" if ty.result => {
                return Ok(Some((receiver.to_owned(), ty)));
            }
            "unwrap_or_else" if ty.result => {
                let closure = arg(args, 0, call)?;
                let value = self.temp();
                let error = self.temp();
                let result = self.temp();
                self.line(&format!("local {value}, {error} = {receiver}"));
                self.line(&format!("local {result} = {value}"));
                self.line(&format!("if {error} ~= nil then"));
                self.indent = self.indent.saturating_add(1);
                let (text, fallback_ty) =
                    self.invoke_arg(closure, std::slice::from_ref(&error))?;
                self.line(&format!("{result} = {text}"));
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                return Ok(Some((
                    result,
                    Ty::new(wider(ty, fallback_ty).base),
                )));
            }
            "map" | "and_then" => {
                let closure = arg(args, 0, call)?;
                let value = self.temp();
                let result = self.temp();
                self.line(&format!("local {value} = {receiver}"));
                self.line(&format!("local {result}"));
                self.line(&format!("if {value} ~= nil then"));
                self.indent = self.indent.saturating_add(1);
                let (text, value_ty) =
                    self.invoke_arg(closure, std::slice::from_ref(&value))?;
                self.line(&format!("{result} = {text}"));
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                return Ok(Some((
                    result,
                    Ty {
                        optional: true,
                        ..value_ty
                    },
                )));
            }
            "filter" => {
                let closure = arg(args, 0, call)?;
                let value = self.temp();
                let result = self.temp();
                self.line(&format!("local {value} = {receiver}"));
                self.line(&format!("local {result}"));
                self.line(&format!("if {value} ~= nil then"));
                self.indent = self.indent.saturating_add(1);
                let (text, _) =
                    self.invoke_arg(closure, std::slice::from_ref(&value))?;
                self.line(&format!("if {text} then {result} = {value} end"));
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                return Ok(Some((result, Ty::opt(ty.base))));
            }
            "is_some_and" => {
                let closure = arg(args, 0, call)?;
                let value = self.temp();
                let result = self.temp();
                self.line(&format!("local {value} = {receiver}"));
                self.line(&format!("local {result} = false"));
                self.line(&format!("if {value} ~= nil then"));
                self.indent = self.indent.saturating_add(1);
                let (text, _) =
                    self.invoke_arg(closure, std::slice::from_ref(&value))?;
                self.line(&format!("{result} = {text}"));
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                return Ok(Some((result, Ty::BOOL)));
            }
            "map_or" => {
                let default = arg(args, 0, call)?;
                let closure = arg(args, 1, call)?;
                let value = self.temp();
                let result = self.temp();
                self.line(&format!("local {value} = {receiver}"));
                let (fallback, fallback_ty) = self.expr(default)?;
                self.line(&format!("local {result} = {fallback}"));
                self.line(&format!("if {value} ~= nil then"));
                self.indent = self.indent.saturating_add(1);
                let (text, value_ty) =
                    self.invoke_arg(closure, std::slice::from_ref(&value))?;
                self.line(&format!("{result} = {text}"));
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                return Ok(Some((result, wider(fallback_ty, value_ty))));
            }
            "map_or_else" => {
                let default = arg(args, 0, call)?;
                let closure = arg(args, 1, call)?;
                let value = self.temp();
                let result = self.temp();
                self.line(&format!("local {value} = {receiver}"));
                self.line(&format!("local {result}"));
                self.line(&format!("if {value} == nil then"));
                self.indent = self.indent.saturating_add(1);
                let (fallback, fallback_ty) = self.invoke_arg(default, &[])?;
                self.line(&format!("{result} = {fallback}"));
                self.indent = self.indent.saturating_sub(1);
                self.line("else");
                self.indent = self.indent.saturating_add(1);
                let (text, value_ty) =
                    self.invoke_arg(closure, std::slice::from_ref(&value))?;
                self.line(&format!("{result} = {text}"));
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                return Ok(Some((result, wider(fallback_ty, value_ty))));
            }
            "unwrap_or_else" | "or_else" => {
                let closure = arg(args, 0, call)?;
                let value = self.temp();
                let result = self.temp();
                self.line(&format!("local {value} = {receiver}"));
                self.line(&format!("local {result} = {value}"));
                self.line(&format!("if {value} == nil then"));
                self.indent = self.indent.saturating_add(1);
                let (text, fallback_ty) = self.fallback(closure, ty)?;
                self.line(&format!("{result} = {text}"));
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                return Ok(Some((result, wider(ty, fallback_ty))));
            }
            "ok_or" => {
                let message = arg(args, 0, call)?;
                let value = self.temp();
                self.line(&format!("local {value} = {receiver}"));
                self.line(&format!("if {value} == nil then"));
                self.indent = self.indent.saturating_add(1);
                let (error, _) = self.expr(message)?;
                self.line(&format!("error(tostring({error}))"));
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                return Ok(Some((
                    value,
                    Ty {
                        result: false,
                        ..ty
                    },
                )));
            }
            "ok_or_else" => {
                let message = arg(args, 0, call)?;
                let value = self.temp();
                self.line(&format!("local {value} = {receiver}"));
                self.line(&format!("if {value} == nil then"));
                self.indent = self.indent.saturating_add(1);
                let (error, _) = self.invoke_arg(message, &[])?;
                self.line(&format!("error(tostring({error}))"));
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                return Ok(Some((
                    value,
                    Ty {
                        result: false,
                        ..ty
                    },
                )));
            }
            _ => {}
        }
        Ok(None)
    }

    /// Iterator terminal calls that fold a collection into one value.
    fn aggregate_method(
        &mut self,
        call: &ExprMethodCall,
    ) -> Result<OptionalValue, Error> {
        let method = call.method.to_string();
        if !matches!(
            method.as_str(),
            "any" | "all" | "sum" | "count" | "for_each" | "find" | "position"
        ) {
            return Ok(None);
        }
        let args = call.args.iter().collect::<Vec<_>>();
        let Some(closure) = args.first().copied() else {
            if method == "count" {
                let (receiver, _) = self.expr(&call.receiver)?;
                return Ok(Some((format!("#{receiver}"), Ty::INT)));
            }
            return Ok(None);
        };
        if matches!(method.as_str(), "find" | "position")
            && matches!(closure, Expr::Closure(_))
        {
            if method == "position" && is_iterator_adaptor(&call.receiver) {
                return Err(Error::at(
                    call.method.span(),
                    "position after an iterator adaptor is not supported by the Luau compiler",
                ));
            }
            if has_flatten_adaptor(&call.receiver) {
                return Err(Error::at(
                    call.method.span(),
                    format!(
                        "{method} after `flatten` is not supported by the Luau compiler"
                    ),
                ));
            }
            let result = self.temp();
            self.line(&format!("local {result}"));
            let index = (method == "position").then(|| {
                let index = self.temp();
                self.line(&format!("local {index} = 0"));
                index
            });
            let body = |emitter: &mut Self, value: &str| -> Result<(), Error> {
                if let Some(index) = &index {
                    emitter.line(&format!("{index} = {index} + 1"));
                }
                let (text, _) = emitter.invoke_arg(
                    closure,
                    std::slice::from_ref(&value.to_owned()),
                )?;
                emitter.line(&format!("if {text} then"));
                emitter.indent = emitter.indent.saturating_add(1);
                let found = index.as_deref().unwrap_or(value);
                emitter.line(&format!("{result} = {found}"));
                emitter.line("break");
                emitter.indent = emitter.indent.saturating_sub(1);
                emitter.line("end");
                Ok(())
            };
            self.emit_iter(&call.receiver, &body)?;
            let ty = if index.is_some() {
                Ty::OPT_INT
            } else {
                Ty::OPT_VALUE
            };
            return Ok(Some((result, ty)));
        }
        if matches!(method.as_str(), "find" | "position") {
            return Ok(None);
        }
        let result = self.temp();
        let initial = match method.as_str() {
            "all" => "true",
            "sum" | "count" => "0",
            "any" => "false",
            _ => "nil",
        };
        if method == "for_each" {
            let body = |emitter: &mut Self, value: &str| -> Result<(), Error> {
                let (text, _) = emitter.invoke_arg(
                    closure,
                    std::slice::from_ref(&value.to_owned()),
                )?;
                if !text.is_empty() {
                    emitter.line(&text);
                }
                Ok(())
            };
            self.emit_iter(&call.receiver, &body)?;
            return Ok(Some((String::new(), Ty::VOID)));
        }
        self.line(&format!("local {result} = {initial}"));
        let body = |emitter: &mut Self, value: &str| -> Result<(), Error> {
            let (text, _) = emitter
                .invoke_arg(closure, std::slice::from_ref(&value.to_owned()))?;
            match method.as_str() {
                "any" => {
                    emitter.line(&format!("if {text} then"));
                    emitter.indent = emitter.indent.saturating_add(1);
                    emitter.line(&format!("{result} = true"));
                    emitter.line("break");
                    emitter.indent = emitter.indent.saturating_sub(1);
                    emitter.line("end");
                }
                "all" => {
                    emitter.line(&format!("if not ({text}) then"));
                    emitter.indent = emitter.indent.saturating_add(1);
                    emitter.line(&format!("{result} = false"));
                    emitter.line("break");
                    emitter.indent = emitter.indent.saturating_sub(1);
                    emitter.line("end");
                }
                "sum" => {
                    emitter.line(&format!("{result} = {result} + ({text})"));
                }
                _ => {
                    emitter.line(&format!("{result} = {result} + 1"));
                }
            }
            Ok(())
        };
        self.emit_iter(&call.receiver, &body)?;
        let ty = match method.as_str() {
            "any" | "all" => Ty::BOOL,
            "count" => Ty::INT,
            _ => Ty::FLOAT,
        };
        Ok(Some((result, ty)))
    }

    fn collect_method(
        &mut self,
        method: &ExprMethodCall,
    ) -> Result<OptionalValue, Error> {
        let result = self.temp();
        self.line(&format!("local {result} = {{}}"));
        let insert = |emitter: &mut Self, value: &str| -> Result<(), Error> {
            emitter.line(&format!("{result}[#{result} + 1] = {value}"));
            Ok(())
        };
        self.emit_iter(&method.receiver, &insert)?;
        Ok(Some((result, Ty::LIST)))
    }

    #[expect(
        clippy::type_complexity,
        reason = "the iterator callback is local to this module"
    )]
    fn emit_iter(
        &mut self,
        expr: &Expr,
        body: &dyn Fn(&mut Self, &str) -> Result<(), Error>,
    ) -> Result<(), Error> {
        match strip_reference(expr) {
            Expr::Paren(paren) => return self.emit_iter(&paren.expr, body),
            Expr::Group(group) => return self.emit_iter(&group.expr, body),
            Expr::MethodCall(call) => match call.method.to_string().as_str() {
                "map" | "filter_map" => {
                    let skip_nil = call.method == "filter_map";
                    let closure =
                        arg(&call.args.iter().collect::<Vec<_>>(), 0, call)?;
                    let inner = |emitter: &mut Self,
                                 value: &str|
                     -> Result<(), Error> {
                        let (text, _) = emitter.invoke_arg(
                            closure,
                            std::slice::from_ref(&value.to_owned()),
                        )?;
                        if text.is_empty() {
                            return Ok(());
                        }
                        if !skip_nil {
                            return body(emitter, &format!("({text})"));
                        }
                        let mapped = emitter.temp();
                        emitter.line(&format!("local {mapped} = ({text})"));
                        emitter.line(&format!("if {mapped} ~= nil then"));
                        emitter.indent = emitter.indent.saturating_add(1);
                        body(emitter, &mapped)?;
                        emitter.indent = emitter.indent.saturating_sub(1);
                        emitter.line("end");
                        Ok(())
                    };
                    return self.emit_iter(&call.receiver, &inner);
                }
                "filter" => {
                    let closure =
                        arg(&call.args.iter().collect::<Vec<_>>(), 0, call)?;
                    let inner = |emitter: &mut Self,
                                 value: &str|
                     -> Result<(), Error> {
                        let (condition, _) = emitter.invoke_arg(
                            closure,
                            std::slice::from_ref(&value.to_owned()),
                        )?;
                        emitter.line(&format!("if {condition} then"));
                        emitter.indent = emitter.indent.saturating_add(1);
                        body(emitter, value)?;
                        emitter.indent = emitter.indent.saturating_sub(1);
                        emitter.line("end");
                        Ok(())
                    };
                    return self.emit_iter(&call.receiver, &inner);
                }
                "split" => {
                    let args = call.args.iter().collect::<Vec<_>>();
                    let (text, _) = self.expr(&call.receiver)?;
                    let separator = self.expr(arg(&args, 0, call)?)?.0;
                    return self.emit_table(
                        &format!("string.split({text}, {separator})"),
                        false,
                        false,
                        body,
                    );
                }
                "split_whitespace" => {
                    let (text, _) = self.expr(&call.receiver)?;
                    let words = self.whitespace_split(&text);
                    return self.emit_table(&words, false, false, body);
                }
                "chars" | "bytes" => {
                    let (text, _) = self.expr(&call.receiver)?;
                    return self.emit_table(
                        &format!("string.split({text}, \"\")"),
                        false,
                        false,
                        body,
                    );
                }
                "lines" => {
                    let (text, _) = self.expr(&call.receiver)?;
                    let lines = self.split_lines(&text);
                    return self.emit_table(&lines, false, false, body);
                }
                "iter" | "into_iter" | "values" => {
                    let (source, ty) = self.expr(&call.receiver)?;
                    let map = ty.base == Base::Table;
                    return self.emit_table(&source, false, map, body);
                }
                "copied" | "cloned" => {
                    return self.emit_iter(&call.receiver, body);
                }
                "keys" => {
                    let (source, ty) = self.expr(&call.receiver)?;
                    let map = ty.base == Base::Table;
                    return self.emit_table(&source, true, map, body);
                }
                "skip" => {
                    let args = call.args.iter().collect::<Vec<_>>();
                    let (count, _) = self.expr(arg(&args, 0, call)?)?;
                    let remaining = self.temp();
                    self.line(&format!("local {remaining} = {count}"));
                    let inner = |emitter: &mut Self,
                                 value: &str|
                     -> Result<(), Error> {
                        emitter.line(&format!("if {remaining} > 0 then"));
                        emitter.indent = emitter.indent.saturating_add(1);
                        emitter.line(&format!("{remaining} = {remaining} - 1"));
                        emitter.indent = emitter.indent.saturating_sub(1);
                        emitter.line("else");
                        emitter.indent = emitter.indent.saturating_add(1);
                        body(emitter, value)?;
                        emitter.indent = emitter.indent.saturating_sub(1);
                        emitter.line("end");
                        Ok(())
                    };
                    return self.emit_iter(&call.receiver, &inner);
                }
                "take" => {
                    let args = call.args.iter().collect::<Vec<_>>();
                    let (count, _) = self.expr(arg(&args, 0, call)?)?;
                    let left = self.temp();
                    self.line(&format!("local {left} = {count}"));
                    let inner = |emitter: &mut Self,
                                 value: &str|
                     -> Result<(), Error> {
                        emitter.line(&format!("if {left} <= 0 then break end"));
                        emitter.line(&format!("{left} = {left} - 1"));
                        body(emitter, value)
                    };
                    return self.emit_iter(&call.receiver, &inner);
                }
                "flatten" => {
                    let (source, ty) = self.expr(&call.receiver)?;
                    if ty.optional {
                        return self.emit_table(
                            &format!("({source} or {{}})"),
                            false,
                            false,
                            body,
                        );
                    }
                    if ty.base == Base::List {
                        let inner =
                            |emitter: &mut Self,
                             value: &str|
                             -> Result<(), Error> {
                                emitter.emit_table(value, false, false, body)
                            };
                        return self.emit_iter(&call.receiver, &inner);
                    }
                }
                "rsplit" => {
                    let args = call.args.iter().collect::<Vec<_>>();
                    let (text, _) = self.expr(&call.receiver)?;
                    let separator = self.expr(arg(&args, 0, call)?)?.0;
                    let parts = Self::reverse_split(&text, &separator);
                    return self.emit_table(&parts, false, false, body);
                }
                _ => {}
            },
            other => {
                let (source, _) = self.expr(other)?;
                return self.emit_table(&source, false, false, body);
            }
        }
        Err(Error::at(
            expr.span(),
            "iterator is not supported by the Luau compiler",
        ))
    }

    #[expect(
        clippy::type_complexity,
        reason = "the iterator callback is local to this module"
    )]
    fn emit_table(
        &mut self,
        source: &str,
        keys: bool,
        map: bool,
        body: &dyn Fn(&mut Self, &str) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let item = self.temp();
        if map {
            if keys {
                self.line(&format!("for {item} in pairs({source}) do"));
            } else {
                self.line(&format!("for _, {item} in pairs({source}) do"));
            }
            self.indent = self.indent.saturating_add(1);
            let result = body(self, &item);
            self.indent = self.indent.saturating_sub(1);
            self.line("end");
            return result;
        }
        let table = self.temp();
        let index = self.temp();
        self.line(&format!("local {table} = {source}"));
        self.line(&format!("for {index} = {table}.__next or 1, #{table} do"));
        self.indent = self.indent.saturating_add(1);
        let result = body(self, &format!("{table}[{index}]"));
        self.indent = self.indent.saturating_sub(1);
        self.line("end");
        result
    }

    fn whitespace_split(&self, text: &str) -> String {
        format!(
            "(function() local __words = {{}}; for __word in string.gmatch({text}, \"%S+\") do __words[#__words + 1] = __word end; __words.__next = 1; return __words end)()"
        )
    }

    fn reverse_split(text: &str, separator: &str) -> String {
        format!(
            "(function() local __parts = string.split({text}, {separator}); local __out = {{}}; for __i = #__parts, 1, -1 do __out[#__out + 1] = __parts[__i] end; __out.__next = 1; return __out end)()"
        )
    }

    fn split_lines(&self, text: &str) -> String {
        format!(
            "(function() local __text = string.gsub({text}, \"\\r\\n\", \"\\n\"); if #__text == 0 then return {{}} end; local __lines = string.split(__text, \"\\n\"); if string.sub(__text, -1) == \"\\n\" then table.remove(__lines) end; __lines.__next = 1; return __lines end)()"
        )
    }

    pub(super) fn class_of_receiver(&self, expr: &Expr) -> Option<String> {
        let Expr::Path(path) = strip_reference(expr) else {
            return None;
        };
        let name = path.path.get_ident()?.to_string();
        self.classes.get(&name).cloned()
    }

    fn method_call(
        &mut self,
        call: &ExprMethodCall,
        receiver: &str,
        class: &str,
    ) -> Result<OptionalValue, Error> {
        let Some((module, function)) = self.program.resolve_method(
            &self.module,
            class,
            &call.method.to_string(),
        ) else {
            return Ok(None);
        };
        let function = function.clone();
        let helper = Helper {
            module,
            name: format!("{class}::{}", call.method),
            function,
            class: Some(class.to_owned()),
        };
        let returns = return_kind(&helper.function)?;
        let mode = match returns {
            Returns::Value(_) => Inline::Value,
            Returns::Unit => Inline::Statement,
            Returns::Ops | Returns::VecOps => {
                return Err(Error::at(
                    call.span(),
                    "methods returning ops are not supported in expression position",
                ));
            }
        };
        let mut bindings = vec![Binding::Local {
            name: "self".to_owned(),
            text: receiver.to_owned(),
            ty: Ty::VALUE,
            container: StaticKind::Value,
        }];
        let args = call.args.iter().collect::<Vec<_>>();
        let params = typed_args(&helper.function);
        if params.len() != args.len() {
            return Err(Error::at(
                call.span(),
                format!(
                    "`{class}::{}` takes {} arguments but {} were passed",
                    call.method,
                    params.len(),
                    args.len()
                ),
            ));
        }
        for ((pattern, ty), argument) in params.iter().zip(args.iter()) {
            bindings.push(self.binding(pattern, ty, argument)?);
        }
        self.self_class = Some(class.to_owned());
        let result = self.inline_resolved(helper, mode, call.span(), bindings);
        self.self_class = None;
        result
    }

    pub(super) fn method(
        &mut self,
        call: &ExprMethodCall,
    ) -> Result<(String, Ty), Error> {
        if call.method == "collect"
            && let Some(value) = self.collect_method(call)?
        {
            return Ok(value);
        }
        if let Some(value) = self.aggregate_method(call)? {
            return Ok(value);
        }
        let method = call.method.to_string();
        let args = call.args.iter().collect::<Vec<_>>();
        if self.is_ctx_expr(&call.receiver) {
            return self.context_method(call, &method, &args);
        }
        if is_config_expr(&call.receiver) && method == "get" {
            let (key, _) = self.expr(arg(&args, 0, call)?)?;
            return Ok((
                self.coerced_string(format!("noctalia.getConfig({key})"), true),
                Ty::OPT_VALUE,
            ));
        }
        if let Some((lua, kind)) = self.static_receiver(&call.receiver)? {
            return self.static_method(&lua, kind, call, &args);
        }
        if let Some(class) = self.class_of_receiver(&call.receiver) {
            let receiver = self.expr(&call.receiver)?.0;
            if let Some(value) = self.method_call(call, &receiver, &class)? {
                return Ok(value);
            }
        }
        let (receiver, ty) = self.expr(&call.receiver)?;
        let receiver = if receiver.starts_with('{') {
            format!("({receiver})")
        } else {
            receiver
        };
        if let Some(value) =
            self.combinator(call, &method, &args, &receiver, ty)?
        {
            return Ok(value);
        }
        match method.as_str() {
            "to_string" if ty.base == Base::Value => {
                Ok((format!("json.encode({receiver})"), Ty::STR))
            }
            "to_string" => Ok((format!("tostring({receiver})"), Ty::STR)),
            "into_owned" | "to_owned" | "clone" | "as_ref" | "as_deref"
            | "cloned" | "into_inner" | "into" | "as_mut" | "as_slice"
            | "as_bytes" | "into_bytes" | "copied" | "peekable"
            | "iter_mut" => Ok((receiver, ty)),
            "first" => Ok((format!("({receiver}[1])"), Ty::OPT_VALUE)),
            "as_object_mut" => Ok((
                format!(
                    "(type({receiver}) == \"table\" and {receiver} or nil)"
                ),
                Ty::OPT_VALUE,
            )),
            "push_str" => {
                let value = self.expr(arg(&args, 0, call)?)?.0;
                self.line(&format!("{receiver} = {receiver} .. ({value})"));
                Ok((String::new(), Ty::VOID))
            }
            "pop" => Ok((format!("table.remove({receiver})"), Ty::OPT_VALUE)),
            "push_back" => {
                let value = self.expr(arg(&args, 0, call)?)?.0;
                Ok((format!("table.insert({receiver}, {value})"), Ty::VOID))
            }
            "find" if !matches!(args.first(), Some(Expr::Closure(_))) => {
                let needle = self.expr(arg(&args, 0, call)?)?.0;
                if ty.base != Base::Str {
                    return Err(Error::at(
                        call.method.span(),
                        "find() needs a string receiver",
                    ));
                }
                Ok((
                    format!(
                        "(function() local __at = string.find({receiver}, {needle}, 1, true); if __at then return __at - 1 end end)()"
                    ),
                    Ty::OPT_INT,
                ))
            }
            "is_nan" => Ok((format!("({receiver} ~= {receiver})"), Ty::BOOL)),
            "div_euclid" => {
                let other = self.expr(arg(&args, 0, call)?)?.0;
                Ok((format!("math.floor({receiver} / {other})"), Ty::INT))
            }
            "sin" | "cos" => {
                Ok((format!("math.{method}({receiver})"), Ty::FLOAT))
            }
            "is_infinite" => Ok((
                format!(
                    "({receiver} == math.huge or {receiver} == -math.huge)"
                ),
                Ty::BOOL,
            )),
            "is_ascii_hexdigit" if ty.base == Base::Int => Ok((
                format!(
                    "(function() local __byte = {receiver}; return (__byte >= 48 and __byte <= 57) or (__byte >= 65 and __byte <= 70) or (__byte >= 97 and __byte <= 102) end)()"
                ),
                Ty::BOOL,
            )),
            "is_ascii_hexdigit" => Ok((
                format!("(string.find({receiver}, \"^[0-9A-Fa-f]$\") ~= nil)"),
                Ty::BOOL,
            )),
            "rfind" => {
                let needle = self.expr(arg(&args, 0, call)?)?.0;
                if ty.base != Base::Str {
                    return Err(Error::at(
                        call.method.span(),
                        "rfind() needs a string receiver",
                    ));
                }
                Ok((
                    format!(
                        "(function() local __s = {receiver}; local __i = string.find(__s, {needle}, 1, true); local __last; while __i do __last = __i - 1; __i = string.find(__s, {needle}, __i + 1, true) end; return __last end)()"
                    ),
                    Ty::OPT_INT,
                ))
            }
            "split_once" => {
                let separator = self.expr(arg(&args, 0, call)?)?.0;
                if ty.base != Base::Str {
                    return Err(Error::at(
                        call.method.span(),
                        "split_once() needs a string receiver",
                    ));
                }
                Ok((
                    format!(
                        "(function() local __s = {receiver}; local __at = string.find(__s, {separator}, 1, true); if not __at then return nil end; return {{ string.sub(__s, 1, __at - 1), string.sub(__s, __at + #{separator}) }} end)()"
                    ),
                    Ty::opt(Base::List),
                ))
            }
            "rsplit" => {
                let separator = self.expr(arg(&args, 0, call)?)?.0;
                if ty.base != Base::Str {
                    return Err(Error::at(
                        call.method.span(),
                        "rsplit() needs a string receiver",
                    ));
                }
                Ok((Self::reverse_split(&receiver, &separator), Ty::LIST))
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
                let count = self.expr(arg(&args, 0, call)?)?.0;
                Ok((
                    format!(
                        "(function() local __out = {{}}; local __n = math.max(0, {count}); for __i = __n + 1, #{receiver} do __out[#__out + 1] = {receiver}[__i] end; __out.__next = 1; return __out end)()"
                    ),
                    Ty::LIST,
                ))
            }
            "take" if ty.base == Base::List && !ty.optional => {
                let count = self.expr(arg(&args, 0, call)?)?.0;
                Ok((
                    format!(
                        "(function() local __out = {{}}; local __n = math.min(math.max(0, {count}), #{receiver}); for __i = 1, __n do __out[#__out + 1] = {receiver}[__i] end; __out.__next = 1; return __out end)()"
                    ),
                    Ty::LIST,
                ))
            }
            "extend_from_slice" => {
                let (value, _) = self.expr(arg(&args, 0, call)?)?;
                let item = self.temp();
                self.line(&format!(
                    "for _, {item} in ipairs({value}) do table.insert({receiver}, {item}) end"
                ));
                Ok((String::new(), Ty::VOID))
            }
            "trunc" => Ok((
                format!(
                    "(({receiver} >= 0) and math.floor({receiver}) or math.ceil({receiver}))"
                ),
                Ty::FLOAT,
            )),
            "truncate" if ty.base == Base::List && !ty.optional => {
                let length = self.expr(arg(&args, 0, call)?)?.0;
                self.line(&format!(
                    "for __i = #{receiver}, ({length}) + 1, -1 do table.remove({receiver}, __i) end"
                ));
                Ok((String::new(), Ty::VOID))
            }
            "match_indices" if ty.base == Base::Str => {
                let needle = self.expr(arg(&args, 0, call)?)?.0;
                Ok((
                    format!(
                        "(function() local __s, __needle = {receiver}, {needle}; local __out = {{}}; local __at = string.find(__s, __needle, 1, true); while __at do __out[#__out + 1] = {{ __at - 1, string.sub(__s, __at, __at + #__needle - 1) }}; __at = string.find(__s, __needle, __at + 1, true) end; return __out end)()"
                    ),
                    Ty::LIST,
                ))
            }
            "retain" => {
                let closure = arg(&args, 0, call)?;
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
            "entry" => Err(Error::at(
                call.method.span(),
                "HashMap entry API is not supported by the Luau compiler",
            )),
            "round" => Ok((format!("math.round({receiver})"), Ty::FLOAT)),
            "then_some" => {
                let value = self.expr(arg(&args, 0, call)?)?.0;
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
                let closure = arg(&args, 0, call)?;
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
                let other = self.expr(arg(&args, 0, call)?)?.0;
                let value = self.temp();
                self.line(&format!("local {value} = {receiver}"));
                Ok((
                    format!(
                        "(function() if {value} ~= nil then return {value} end return {other} end)()"
                    ),
                    ty,
                ))
            }
            "trim_start" => Ok((
                format!("string.gsub({receiver}, \"^%s+\", \"\")"),
                Ty::STR,
            )),
            "trim_end" => Ok((
                format!("string.gsub({receiver}, \"%s+$\", \"\")"),
                Ty::STR,
            )),
            "strip_suffix" => {
                let suffix = self.expr(arg(&args, 0, call)?)?.0;
                let value = self.temp();
                self.line(&format!("local {value} = {receiver}"));
                Ok((
                    format!(
                        "(string.sub({value}, -#{suffix}) == {suffix} and string.sub({value}, 1, #{value} - #{suffix}) or nil)"
                    ),
                    Ty::OPT_STR,
                ))
            }
            "is_ascii_uppercase" => Ok((
                format!("(string.find({receiver}, \"^%u+$\") ~= nil)"),
                Ty::BOOL,
            )),
            "is_ascii_lowercase" => Ok((
                format!("(string.find({receiver}, \"^%l+$\") ~= nil)"),
                Ty::BOOL,
            )),
            "is_ascii_alphabetic" => Ok((
                format!("(string.find({receiver}, \"^%a+$\") ~= nil)"),
                Ty::BOOL,
            )),
            "is_ascii_alphanumeric" => Ok((
                format!("(string.find({receiver}, \"^%w+$\") ~= nil)"),
                Ty::BOOL,
            )),
            "is_ascii_digit" => Ok((
                format!("(string.find({receiver}, \"^%d+$\") ~= nil)"),
                Ty::BOOL,
            )),
            "is_ascii_whitespace" | "is_whitespace" => Ok((
                format!("(string.find({receiver}, \"^%s+$\") ~= nil)"),
                Ty::BOOL,
            )),
            "is_char_boundary" | "char_indices" => Err(Error::at(
                call.method.span(),
                format!(
                    "`{method}` has no Luau equivalent and is not supported by the Luau compiler"
                ),
            )),
            "to_string_lossy" | "display" => Ok((receiver, Ty::STR)),
            "as_str" => Ok((receiver, Ty::OPT_STR)),
            "as_i64" | "as_u64" => Ok((receiver, Ty::OPT_INT)),
            "as_f64" => Ok((receiver, Ty::OPT_FLOAT)),
            "as_bool" => Ok((receiver, Ty::OPT_BOOL)),
            "as_array" => Ok((receiver, Ty::OPT_LIST)),
            "is_some" => Ok((format!("({receiver} ~= nil)"), Ty::BOOL)),
            "is_none" => Ok((format!("({receiver} == nil)"), Ty::BOOL)),
            "is_ok" => Ok((format!("({receiver} ~= nil)"), Ty::BOOL)),
            "is_err" => Ok((format!("({receiver} == nil)"), Ty::BOOL)),
            "is_null" => Ok((format!("({receiver} == nil)"), Ty::BOOL)),
            "is_finite" => Ok((
                format!(
                    "({receiver} == {receiver} and {receiver} ~= math.huge and {receiver} ~= -math.huge)"
                ),
                Ty::BOOL,
            )),
            "floor" | "ceil" | "abs" | "sqrt" => {
                Ok((format!("math.{method}({receiver})"), Ty::FLOAT))
            }
            "fract" => Ok((
                format!("({receiver} - math.floor({receiver}))"),
                Ty::FLOAT,
            )),
            "clamp" => {
                let min = self.expr(arg(&args, 0, call)?)?.0;
                let max = self.expr(arg(&args, 1, call)?)?.0;
                Ok((format!("math.clamp({receiver}, {min}, {max})"), Ty::FLOAT))
            }
            "max" | "min" => {
                let other = self.expr(arg(&args, 0, call)?)?.0;
                Ok((format!("math.{method}({receiver}, {other})"), Ty::FLOAT))
            }
            "rem_euclid" => {
                let other = self.expr(arg(&args, 0, call)?)?.0;
                Ok((
                    format!("(({receiver} % {other}) + {other}) % {other}"),
                    Ty::FLOAT,
                ))
            }
            "join" if args.len() == 1 => {
                let part = self.expr(arg(&args, 0, call)?)?.0;
                if ty.base == Base::List {
                    return Ok((
                        format!("table.concat({receiver}, {part})"),
                        Ty::STR,
                    ));
                }
                if ty.base != Base::Str {
                    self.uses_join = true;
                    return Ok((format!("join({receiver}, {part})"), Ty::STR));
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
            "strip_prefix" => {
                let prefix = self.expr(arg(&args, 0, call)?)?.0;
                let value = self.temp();
                self.line(&format!("local {value} = {receiver}"));
                Ok((
                    format!(
                        "(string.sub({value}, 1, #{prefix}) == {prefix} and string.sub({value}, #{prefix} + 1) or nil)"
                    ),
                    Ty::OPT_STR,
                ))
            }
            "trim_matches" | "trim_start_matches" | "trim_end_matches" => {
                let argument = arg(&args, 0, call)?;
                let pattern = self.char_class(argument, call)?;
                let leading = quote(&format!("^{pattern}+"));
                let trailing = quote(&format!("{pattern}+$"));
                let trimmed =
                    format!("string.gsub({receiver}, {leading}, \"\")");
                if method == "trim_start_matches" {
                    return Ok((trimmed, Ty::STR));
                }
                let base = if method == "trim_end_matches" {
                    receiver.clone()
                } else {
                    trimmed
                };
                Ok((format!("string.gsub({base}, {trailing}, \"\")"), Ty::STR))
            }
            "to_ascii_uppercase" => {
                Ok((format!("string.upper({receiver})"), Ty::STR))
            }
            "to_ascii_lowercase" => {
                Ok((format!("string.lower({receiver})"), Ty::STR))
            }
            "starts_with" => {
                let prefix = self.expr(arg(&args, 0, call)?)?.0;
                Ok((
                    format!(
                        "(string.sub({receiver}, 1, #{prefix}) == {prefix})"
                    ),
                    Ty::BOOL,
                ))
            }
            "ends_with" => {
                let suffix = self.expr(arg(&args, 0, call)?)?.0;
                Ok((
                    format!("(string.sub({receiver}, -#{suffix}) == {suffix})"),
                    Ty::BOOL,
                ))
            }
            "as_object" => Ok((
                format!(
                    "(type({receiver}) == \"table\" and {receiver} or nil)"
                ),
                Ty::OPT_VALUE,
            )),
            "replace" => {
                let from = self.expr(arg(&args, 0, call)?)?.0;
                let to = self.expr(arg(&args, 1, call)?)?.0;
                self.uses_replace = true;
                Ok((format!("replace_all({receiver}, {from}, {to})"), Ty::STR))
            }
            "duration_since" | "as_millis" => Ok((receiver, Ty::INT)),
            "as_secs" => {
                Ok((format!("math.floor({receiver} / 1000)"), Ty::INT))
            }
            "as_nanos" => Ok((format!("({receiver} * 1000000)"), Ty::INT)),
            "iter" | "into_iter" | "values" => Ok((receiver, ty)),
            "split" => {
                let delimiter = self.expr(arg(&args, 0, call)?)?.0;
                Ok((
                    format!(
                        "(function() local __parts = string.split({receiver}, {delimiter}); __parts.__next = 1; return __parts end)()"
                    ),
                    Ty::LIST,
                ))
            }
            "split_whitespace" => {
                Ok((self.whitespace_split(&receiver), Ty::LIST))
            }
            "next" => {
                let value = self.temp();
                self.line(&format!(
                    "local {value} = {receiver}[{receiver}.__next]"
                ));
                self.line(&format!(
                    "if {value} ~= nil then {receiver}.__next = {receiver}.__next + 1 end"
                ));
                Ok((value, Ty::opt(ty.base)))
            }
            "as_os_str" | "to_str" | "to_vec" => Ok((receiver, Ty::VALUE)),
            "bytes" | "chars" => {
                Ok((format!("string.split({receiver}, \"\")"), Ty::LIST))
            }
            "lines" => Ok((self.split_lines(&receiver), Ty::LIST)),
            "clear" => {
                self.line(&format!("table.clear({receiver})"));
                Ok((String::new(), Ty::VOID))
            }
            "insert" if args.len() == 1 => {
                let value = self.expr(arg(&args, 0, call)?)?.0;
                self.line(&format!("{receiver}[{value}] = true"));
                Ok((String::new(), Ty::VOID))
            }
            "insert" if args.len() == 2 => {
                let key = self.expr(arg(&args, 0, call)?)?.0;
                let value = self.expr(arg(&args, 1, call)?)?.0;
                if ty.base != Base::Table
                    && key.chars().all(|character| character.is_ascii_digit())
                {
                    self.line(&format!(
                        "table.insert({receiver}, ({key}) + 1, {value})"
                    ));
                } else {
                    self.line(&format!("{receiver}[{key}] = {value}"));
                }
                Ok((String::new(), Ty::VOID))
            }
            "remove" if args.len() == 1 => {
                let (key, key_ty) = self.expr(arg(&args, 0, call)?)?;
                let removed = self.temp();
                if ty.base == Base::Table {
                    self.line(&format!("local {removed} = {receiver}[{key}]"));
                    self.line(&format!("{receiver}[{key}] = nil"));
                    return Ok((removed, Ty::OPT_VALUE));
                }
                if ty.base == Base::List && key_ty.base == Base::Int {
                    self.line(&format!(
                        "local {removed} = table.remove({receiver}, ({key}) + 1)"
                    ));
                    return Ok((removed, Ty::VALUE));
                }
                if key_ty.base != Base::Int {
                    self.line(&format!("local {removed} = {receiver}[{key}]"));
                    self.line(&format!("{receiver}[{key}] = nil"));
                    return Ok((removed, Ty::OPT_VALUE));
                }
                Err(Error::at(
                    call.method.span(),
                    "remove() needs a list with an integer index or a map with the key",
                ))
            }
            "is_object" | "is_array" => {
                Ok((format!("(type({receiver}) == \"table\")"), Ty::BOOL))
            }
            "is_string" => {
                Ok((format!("(type({receiver}) == \"string\")"), Ty::BOOL))
            }
            "is_number" => {
                Ok((format!("(type({receiver}) == \"number\")"), Ty::BOOL))
            }
            "is_boolean" => {
                Ok((format!("(type({receiver}) == \"boolean\")"), Ty::BOOL))
            }
            "expect" => {
                let (message, _) = self.expr(arg(&args, 0, call)?)?;
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
                Ok((receiver, ty))
            }
            "ok" if ty.result => {
                let value = self.temp();
                let error = self.temp();
                self.line(&format!("local {value}, {error} = {receiver}"));
                Ok((value, Ty::opt(ty.base)))
            }
            "ok" => Ok((
                receiver,
                Ty {
                    optional: true,
                    ..ty
                },
            )),
            "get_or_insert" | "get_or_insert_with" => {
                let value = arg(&args, 0, call)?;
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
                Ok((receiver, value_ty))
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
                let (default, default_ty) = self.expr(arg(&args, 0, call)?)?;
                if !ty.result
                    && (ty.base == Base::Bool || default_ty.base == Base::Bool)
                {
                    self.uses_bool_or = true;
                    return Ok((
                        format!("bool_or({receiver}, {default})"),
                        Ty::BOOL,
                    ));
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
                    return Ok((result, Ty::new(wider(ty, default_ty).base)));
                }
                Ok((
                    format!("({receiver} or {default})"),
                    Ty::new(wider(ty, default_ty).base),
                ))
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
                    return Ok((result, Ty::new(ty.base)));
                }
                Ok((format!("({receiver} or {default})"), Ty::new(ty.base)))
            }
            "len" => Ok((format!("#{receiver}"), Ty::INT)),
            "is_empty" => Ok((format!("(#{receiver} == 0)"), Ty::BOOL)),
            "get" => {
                let (key, key_ty) = self.expr(arg(&args, 0, call)?)?;
                if ty.base != Base::Table && key_ty.base == Base::Int {
                    Ok((format!("{receiver}[({key}) + 1]"), Ty::OPT_VALUE))
                } else {
                    Ok((format!("{receiver}[{key}]"), Ty::OPT_VALUE))
                }
            }
            "trim" => Ok((format!("string.trim({receiver})"), Ty::STR)),
            "to_lowercase" => {
                Ok((format!("string.lower({receiver})"), Ty::STR))
            }
            "to_uppercase" => {
                Ok((format!("string.upper({receiver})"), Ty::STR))
            }
            "contains" => {
                let (needle, _) = self.expr(arg(&args, 0, call)?)?;
                Ok((
                    format!(
                        "(string.find({receiver}, {needle}, 1, true) ~= nil)"
                    ),
                    Ty::BOOL,
                ))
            }
            "push" if ty.base == Base::Ops => {
                let value = arg(&args, 0, call)?;
                if is_op_expr(value) {
                    self.op_expr(value)?;
                } else {
                    let (text, _) = self.expr(value)?;
                    self.line(&format!("table.insert({receiver}, {text})"));
                }
                Ok((String::new(), Ty::VOID))
            }
            "extend" => {
                let (value, _) = self.expr(arg(&args, 0, call)?)?;
                let item = self.temp();
                self.line(&format!(
                    "for _, {item} in ipairs({value}) do {receiver}[#({receiver}) + 1] = {item} end"
                ));
                Ok((String::new(), Ty::VOID))
            }
            "push" => {
                let (value, _) = self.expr(arg(&args, 0, call)?)?;
                Ok((format!("table.insert({receiver}, {value})"), Ty::VOID))
            }
            _ => Err(Error::at(
                call.method.span(),
                format!(
                    "method `{method}` is not supported by the Luau compiler"
                ),
            )),
        }
    }

    fn context_method(
        &mut self,
        call: &ExprMethodCall,
        method: &str,
        args: &[&Expr],
    ) -> Result<(String, Ty), Error> {
        match method {
            "config_str" => {
                let key = self.string_arg(args, 0, call)?;
                let raw = key.trim_matches('"');
                if self.strings.iter().any(|item| item == raw) {
                    return Ok((
                        format!("noctalia.getConfig({key})"),
                        Ty::OPT_STR,
                    ));
                }
                Ok((
                    self.coerced_string(
                        format!("noctalia.getConfig({key})"),
                        true,
                    ),
                    Ty::OPT_STR,
                ))
            }
            "config_bool" => {
                self.uses_bool_or = true;
                let key = self.string_arg(args, 0, call)?;
                let (default, _) = self.expr(arg(args, 1, call)?)?;
                Ok((
                    format!(
                        "bool_or(noctalia.getConfig({key}), {default})"
                    ),
                    Ty::BOOL,
                ))
            }
            "payload_str" => {
                let key = self.string_arg(args, 0, call)?;
                let raw = key.trim_matches('"');
                if let Some((_, body_param, _)) = self
                    .payload_params
                    .iter()
                    .find(|(payload_key, _, _)| *payload_key == raw)
                {
                    if !self.params.contains(body_param) {
                        self.params.push(body_param);
                    }
                    return Ok(((*body_param).to_owned(), Ty::OPT_STR));
                }
                self.uses_payload = true;
                Ok((
                    self.coerced_string(format!("payload[{key}]"), false),
                    Ty::OPT_STR,
                ))
            }
            "payload_arg" => {
                self.uses_payload = true;
                let (index, _) = self.expr(arg(args, 0, call)?)?;
                Ok((
                    format!("payload.args[({index}) + 1]"),
                    Ty::OPT_VALUE,
                ))
            }
            "state_value" => {
                self.uses_state = true;
                let key = self.string_arg(args, 0, call)?;
                let ty = turbofish_type(call.turbofish.as_ref());
                Ok((format!("plugin_state[{key}]"), ty))
            }
            "setting" | "setting_value" => {
                let key = self.string_arg(args, 0, call)?;
                Ok((
                    format!("noctalia.getSetting({key})"),
                    Ty::OPT_VALUE,
                ))
            }
            "color" => {
                let role = self.string_arg(args, 0, call)?;
                Ok((format!("noctalia.getColor({role})"), Ty::OPT_VALUE))
            }
            "host" => {
                let path = self.string_arg(args, 0, call)?;
                Ok((host_path(&unquote(&path))?.to_owned(), Ty::OPT_VALUE))
            }
            "clipboard" => Ok((
                "noctalia.clipboardText()".to_owned(),
                Ty::OPT_VALUE,
            )),
            "calendar" => Ok((
                "noctalia.calendarEvents()".to_owned(),
                Ty::OPT_VALUE,
            )),
            "media" => {
                Ok(("noctalia.mediaPlayer()".to_owned(), Ty::OPT_VALUE))
            }
            "stats" => {
                Ok(("noctalia.systemStats()".to_owned(), Ty::OPT_VALUE))
            }
            "nightlight" => Ok((
                "noctalia.nightlight.state".to_owned(),
                Ty::OPT_VALUE,
            )),
            "compositor" => Ok((
                "{ name = noctalia.compositor.name }".to_owned(),
                Ty::VALUE,
            )),
            "bar" => Ok((
                "{ vertical = barWidget.isVertical(), output_name = barWidget.outputName() }"
                    .to_owned(),
                Ty::VALUE,
            )),
            "bar_vertical" => {
                Ok(("barWidget.isVertical()".to_owned(), Ty::BOOL))
            }
            "bar_output_name" => Ok((
                "barWidget.outputName()".to_owned(),
                Ty::OPT_STR,
            )),
            "dark_mode" => {
                Ok(("noctalia.isDarkMode()".to_owned(), Ty::BOOL))
            }
            "plugin_dir" => {
                Ok(("noctalia.pluginDir()".to_owned(), Ty::STR))
            }
            "plugin_data_dir" => Ok((
                "noctalia.pluginDataDir()".to_owned(),
                Ty::OPT_STR,
            )),
            "time_format" => Ok((
                "noctalia.getSetting(\"shell.time_format\")".to_owned(),
                Ty::OPT_STR,
            )),
            "date_format" => Ok((
                "noctalia.getSetting(\"shell.date_format\")".to_owned(),
                Ty::OPT_STR,
            )),
            "tr" => {
                let key = self.expr(arg(args, 0, call)?)?.0;
                let params = arg(args, 1, call)?;
                let params = strip_reference(params);
                let table = self.tr_params(params)?;
                Ok((format!("noctalia.tr({key}, {table})"), Ty::STR))
            }
            "file_exists" => {
                let path = self.path_arg(args, 0, call)?;
                Ok((format!("noctalia.fileExists({path})"), Ty::BOOL))
            }
            "command_exists" => {
                let name = self.string_arg(args, 0, call)?;
                Ok((format!("noctalia.commandExists({name})"), Ty::BOOL))
            }
            "read_file" => {
                let path = self.path_arg(args, 0, call)?;
                Ok((
                    format!("noctalia.readFile({path})"),
                    Ty::result(Base::Str),
                ))
            }
            "write_file" => {
                let path = self.path_arg(args, 0, call)?;
                let contents = self.path_arg(args, 1, call)?;
                Ok((
                    format!("noctalia.writeFile({path}, {contents})"),
                    Ty::VOID,
                ))
            }
            "env" => {
                let key = self.string_arg(args, 0, call)?;
                Ok((format!("noctalia.getenv({key})"), Ty::OPT_STR))
            }
            "expand_path" => {
                let path = self.path_arg(args, 0, call)?;
                Ok((format!("noctalia.expandPath({path})"), Ty::STR))
            }
            "list_dir" => {
                let path = self.path_arg(args, 0, call)?;
                Ok((format!("(noctalia.listDir({path}))"), Ty::LIST))
            }
            "mkdir_all" => {
                let path = self.path_arg(args, 0, call)?;
                Ok((format!("noctalia.mkdirAll({path})"), Ty::VOID))
            }
            "remove_file" => {
                let path = self.path_arg(args, 0, call)?;
                Ok((format!("noctalia.removeFile({path})"), Ty::VOID))
            }
            "format_time" => {
                let format = self.path_arg(args, 0, call)?;
                Ok((format!("noctalia.formatTime({format})"), Ty::STR))
            }
            "format_time_at" => {
                let format = self.path_arg(args, 0, call)?;
                let seconds = match args.get(1) {
                    Some(argument) => {
                        let (text, _) = self.expr(argument)?;
                        format!("({text})")
                    }
                    None => "nil".to_owned(),
                };
                let timezone = match args.get(2) {
                    Some(argument) => {
                        let (text, _) = self.expr(argument)?;
                        format!("({text})")
                    }
                    None => "nil".to_owned(),
                };
                Ok((
                    format!(
                        "noctalia.formatTime({format}, {seconds}, {timezone})"
                    ),
                    Ty::STR,
                ))
            }
            "plugin_data_dir_result" => Ok((
                "(noctalia.pluginDataDir())".to_owned(),
                Ty::OPT_STR,
            )),
            "plugin_dir_result" => {
                Ok(("(noctalia.pluginDir())".to_owned(), Ty::OPT_STR))
            }
            "os_clock" => Ok(("os.clock()".to_owned(), Ty::FLOAT)),
            "os_time" => Ok(("os.time()".to_owned(), Ty::INT)),
            "math_random" => Ok(("math.random()".to_owned(), Ty::FLOAT)),
            "global_state" => {
                let key = self.string_arg(args, 0, call)?;
                Ok((
                    format!("state.get({key})"),
                    Ty::OPT_VALUE,
                ))
            }
            "render_source" => {
                self.uses_source = true;
                Ok(("source".to_owned(), Ty::STR))
            }
            "is_load_event" => {
                let text = match self.bound_event.as_deref() {
                    Some("load") => "true",
                    Some(_) => {
                        self.uses_source = true;
                        "(source == \"load\")"
                    }
                    None => {
                        self.uses_source = true;
                        "(event == \"load\" or source == \"load\")"
                    }
                };
                Ok((text.to_owned(), Ty::BOOL))
            }
            "read_file_result" => {
                let path = self.path_arg(args, 0, call)?;
                Ok((format!("(noctalia.readFile({path}))"), Ty::VALUE))
            }
            "list_dir_result" => {
                let path = self.path_arg(args, 0, call)?;
                Ok((format!("(noctalia.listDir({path}))"), Ty::VALUE))
            }
            "load_font_result" => {
                let path = self.path_arg(args, 0, call)?;
                Ok((format!("(noctalia.loadFont({path}))"), Ty::VALUE))
            }
            "file_info_result" => {
                let path = self.path_arg(args, 0, call)?;
                Ok((format!("(noctalia.fileInfo({path}))"), Ty::VALUE))
            }
            "mkdir_all_result" => {
                let path = self.path_arg(args, 0, call)?;
                Ok((format!("(noctalia.mkdirAll({path}))"), Ty::VALUE))
            }
            "remove_file_result" => {
                let path = self.path_arg(args, 0, call)?;
                Ok((format!("(noctalia.removeFile({path}))"), Ty::VALUE))
            }
            "math_randomseed" => {
                let seed = self.expr(arg(args, 0, call)?)?.0;
                Ok((format!("math.randomseed({seed})"), Ty::VOID))
            }
            "math_random_range" => {
                let min = self.expr(arg(args, 0, call)?)?.0;
                let max = self.expr(arg(args, 1, call)?)?.0;
                Ok((format!("math.random({min}, {max})"), Ty::FLOAT))
            }
            "trp" => {
                let key = self.expr(arg(args, 0, call)?)?.0;
                let count = self.expr(arg(args, 1, call)?)?.0;
                let params = arg(args, 2, call)?;
                let table = self.tr_params(strip_reference(params))?;
                Ok((
                    format!("noctalia.trp({key}, {count}, {table})"),
                    Ty::STR,
                ))
            }
            "query_value" | "query_bool" => {
                let root = self.string_arg(args, 0, call)?;
                let path = self.expr(arg(args, 1, call)?)?.0;
                let values = self.expr(arg(args, 2, call)?)?.0;
                self.uses_query = true;
                let text = format!("host_query({root}, {path}, {values})");
                if method == "query_bool" {
                    return Ok((format!("({text} == true)"), Ty::BOOL));
                }
                Ok((text, Ty::VALUE))
            }
            "fuzzy_score" => {
                let pattern = self.string_arg(args, 0, call)?;
                let text = self.string_arg(args, 1, call)?;
                Ok((
                    format!("noctalia.fuzzyScore({pattern}, {text})"),
                    Ty::OPT_FLOAT,
                ))
            }
            "file_info" => {
                let path = self.path_arg(args, 0, call)?;
                Ok((format!("noctalia.fileInfo({path})"), Ty::VALUE))
            }
            "run_async_start" | "run_stream_start" | "http_start" => {
                if !self.in_statement {
                    return Err(Error::at(
                        call.method.span(),
                        format!(
                            "context method `{method}` blocks in Rust to report acceptance; compiled Luau starts the call and returns true, so its result must be discarded"
                        ),
                    ));
                }
                let (request, _) = self.expr(arg(args, 0, call)?)?;
                let callback = self
                    .callback_marker(args, 1)?
                    .unwrap_or_else(|| "nil".to_owned());
                let head = match method {
                    "run_async_start" => "noctalia.runAsync",
                    "run_stream_start" => "noctalia.runStream",
                    _ => "noctalia.http",
                };
                let line = if method == "run_async_start" {
                    let timeout = match args.get(2) {
                        Some(timeout) => self.option_value(timeout)?,
                        None => "nil".to_owned(),
                    };
                    format!("{head}({request}, {callback}, {timeout})")
                } else {
                    format!("{head}({request}, {callback})")
                };
                self.line(&line);
                Ok((String::new(), Ty::VOID))
            }
            other => Err(Error::at(
                call.method.span(),
                format!(
                    "context method `{other}` is not supported by the Luau compiler"
                ),
            )),
        }
    }

    pub(super) fn coerced_string(
        &mut self,
        value: String,
        allow_bool: bool,
    ) -> String {
        let name = self.temp();
        self.line(&format!("local {name} = {value}"));
        if allow_bool {
            format!(
                "((type({name}) == \"string\" and {name}) or ((type({name}) == \"number\" or type({name}) == \"boolean\") and tostring({name})) or nil)"
            )
        } else {
            format!(
                "((type({name}) == \"string\" and {name}) or (type({name}) == \"number\" and tostring({name})) or nil)"
            )
        }
    }

    fn tr_params(&mut self, expr: &Expr) -> Result<String, Error> {
        let Expr::Array(array) = expr else {
            return Err(Error::at(
                expr.span(),
                "ctx.tr params must be an array of tuples",
            ));
        };
        let mut parts = Vec::new();
        for element in &array.elems {
            let Expr::Tuple(tuple) = element else {
                return Err(Error::at(
                    element.span(),
                    "ctx.tr params must be (name, value) tuples",
                ));
            };
            let mut values = tuple.elems.iter();
            let name = values.next().ok_or_else(|| {
                Error::at(element.span(), "missing parameter name")
            })?;
            let value = values.next().ok_or_else(|| {
                Error::at(element.span(), "missing parameter value")
            })?;
            let name = expr_string(name)?;
            let (value, _) = self.expr(value)?;
            parts.push(format!("{} = {value}", lua_key(&name)));
        }
        Ok(format!("{{ {} }}", parts.join(", ")))
    }

    fn string_arg(
        &mut self,
        args: &[&Expr],
        index: usize,
        call: &ExprMethodCall,
    ) -> Result<String, Error> {
        let value = arg(args, index, call)?;
        let (text, ty) = self.expr(value)?;
        if ty.base != Base::Str {
            return Err(Error::at(
                value.span(),
                "expected a string literal argument",
            ));
        }
        Ok(text)
    }

    /// The Lua character class for a Rust character set, such as a literal, a
    /// `&[char]` constant, or a predicate over one character.
    fn char_class(
        &mut self,
        expr: &Expr,
        call: &ExprMethodCall,
    ) -> Result<String, Error> {
        let chars = self.char_set(expr)?.ok_or_else(|| {
            Error::at(
                call.method.span(),
                "expected a string or char literal argument",
            )
        })?;
        let class = chars
            .chars()
            .map(|character| match character {
                ']' | '-' | '^' | '%' => format!("%{character}"),
                other => other.to_string(),
            })
            .collect::<String>();
        Ok(format!("[{class}]"))
    }

    /// Every character a predicate accepts, when it is a plain set.
    fn char_set(&mut self, expr: &Expr) -> Result<Option<String>, Error> {
        match strip_reference(expr) {
            Expr::Lit(ExprLit {
                lit: Lit::Str(value),
                ..
            }) => Ok(Some(value.value())),
            Expr::Lit(ExprLit {
                lit: Lit::Char(value),
                ..
            }) => Ok(Some(value.value().to_string())),
            Expr::Lit(ExprLit {
                lit: Lit::Byte(value),
                ..
            }) => Ok(Some((value.value() as char).to_string())),
            Expr::Lit(ExprLit {
                lit: Lit::ByteStr(value),
                ..
            }) => Ok(Some(
                value.value().iter().map(|byte| *byte as char).collect(),
            )),
            Expr::Array(array) => {
                let mut chars = String::new();
                for item in &array.elems {
                    match constant(item) {
                        Some((text, _)) => chars.push_str(&unquote(&text)),
                        None => return Ok(None),
                    }
                }
                Ok(Some(chars))
            }
            Expr::Index(index) => self.char_set(&index.expr),
            Expr::Closure(closure) => self.char_set_from_body(&closure.body),
            Expr::Paren(paren) => self.char_set(&paren.expr),
            Expr::Group(group) => self.char_set(&group.expr),
            Expr::Path(path) => {
                let Expr::Path(item) = strip_reference(expr) else {
                    return Ok(None);
                };
                let Some(segments) = path_segments(&item.path) else {
                    return Ok(None);
                };
                if path_segments(&path.path).is_some()
                    && let Some(resolved) =
                        self.program.resolve(&self.module, &segments)
                {
                    let body = resolved.function.block.clone();
                    return self.char_set_from_body(&Expr::Block(
                        syn::ExprBlock {
                            attrs: Vec::new(),
                            label: None,
                            block: *body,
                        },
                    ));
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn char_set_from_body(
        &mut self,
        body: &Expr,
    ) -> Result<Option<String>, Error> {
        match body {
            Expr::Block(block) => match block.block.stmts.as_slice() {
                [Stmt::Expr(expr, _)] => self.char_set_from_body(expr),
                _ => Ok(None),
            },
            Expr::Return(ret) => match &ret.expr {
                Some(expr) => self.char_set_from_body(expr),
                None => Ok(None),
            },
            Expr::Paren(paren) => self.char_set_from_body(&paren.expr),
            Expr::Group(group) => self.char_set_from_body(&group.expr),
            Expr::Binary(binary) if matches!(binary.op, BinOp::Or(_)) => {
                let mut chars = String::new();
                for side in [&binary.left, &binary.right] {
                    match self.char_set_from_body(side)? {
                        Some(found) => chars.push_str(&found),
                        None => return Ok(None),
                    }
                }
                Ok(Some(chars))
            }
            Expr::Macro(mac) if macro_name(mac) == "matches" => {
                let parsed: MatchesArgs = syn::parse2(mac.mac.tokens.clone())?;
                let mut chars = String::new();
                if !pattern_chars(&parsed.pat, &mut chars)? {
                    return Ok(None);
                }
                Ok(Some(chars))
            }
            Expr::MethodCall(call)
                if matches!(
                    call.method.to_string().as_str(),
                    "is_ascii_whitespace" | "is_whitespace"
                ) =>
            {
                Ok(Some(" \t\n\r\u{0b}\u{0c}".to_owned()))
            }
            Expr::MethodCall(call)
                if matches!(
                    call.method.to_string().as_str(),
                    "is_ascii_digit" | "is_digit"
                ) =>
            {
                Ok(Some("0123456789".to_owned()))
            }
            _ => Ok(None),
        }
    }

    /// A path or format argument. Values of unknown type are coerced the same
    /// way `Host` does, so a config field can be passed where a string is
    /// expected.
    fn path_arg(
        &mut self,
        args: &[&Expr],
        index: usize,
        call: &ExprMethodCall,
    ) -> Result<String, Error> {
        let value = arg(args, index, call)?;
        let (text, ty) = self.expr(value)?;
        if ty.base == Base::Str {
            return Ok(text);
        }
        Ok(format!("tostring({text})"))
    }

    pub(super) fn std_constructor(
        &mut self,
        call: &ExprCall,
        args: &[&Expr],
    ) -> Result<OptionalValue, Error> {
        let Some(name) = path_last_name(&call.func) else {
            return Ok(None);
        };
        let parts = name.split("::").collect::<Vec<_>>();
        let variant = if parts.last().is_some_and(|part| *part == "new") {
            parts
                .get(parts.len().saturating_sub(2))
                .copied()
                .unwrap_or("new")
        } else {
            parts.last().copied().unwrap_or("")
        };
        match variant {
            "HashMap" | "BTreeMap" | "Map" => {
                Ok(Some(("{}".to_owned(), Ty::TABLE)))
            }
            "HashSet" | "BTreeSet" => Ok(Some(("{}".to_owned(), Ty::LIST))),
            "Bool" | "String" | "Number"
                if name.starts_with("String::")
                    || name.starts_with("bool::") =>
            {
                let inner = arg(args, 0, call)?;
                self.expr(inner).map(Some)
            }
            "Mutex" | "RwLock" | "Cell" | "RefCell" => {
                let inner = arg(args, 0, call)?;
                let (text, ty) = self.expr(inner)?;
                if variant == "Mutex" || variant == "RwLock" {
                    if let Some(class) = self.pending_class.take() {
                        self.classes.insert(text.clone(), class);
                    }
                    return Ok(Some((text, Ty::result(Base::Value))));
                }
                Ok(Some((text, ty)))
            }
            "OnceLock" | "LazyLock" => Ok(Some(("nil".to_owned(), Ty::VALUE))),
            "node" if name.contains("ui::") => {
                let kind = self.expr(arg(args, 0, call)?)?.0;
                let props = self.expr(arg(args, 1, call)?)?.0;
                let children = self.expr(arg(args, 2, call)?)?.0;
                Ok(Some((
                    format!(
                        "{{ kind = {kind}, props = {props}, children = {children} }}"
                    ),
                    Ty::NODE,
                )))
            }
            "Some" => self.expr(arg(args, 0, call)?).map(Some),
            "Ok" => self.expr(arg(args, 0, call)?).map(Some),
            _ => Ok(None),
        }
    }

    pub(super) fn associated_call(
        &mut self,
        call: &ExprCall,
        args: &[&Expr],
    ) -> Result<OptionalValue, Error> {
        let Expr::Path(path) = &*call.func else {
            return Ok(None);
        };
        let Some(segments) = path_segments(&path.path) else {
            return Ok(None);
        };
        if segments.len() < 2 {
            return Ok(None);
        }
        let method = segments.last().map_or("", String::as_str);
        let class = segments[segments.len() - 2].as_str();
        let Some((module, function)) =
            self.program.resolve_method(&self.module, class, method)
        else {
            return Ok(None);
        };
        let function = function.clone();
        let helper = Helper {
            module,
            name: format!("{class}::{method}"),
            function,
            class: Some(class.to_owned()),
        };
        let mut bindings = Vec::new();
        let params = typed_args(&helper.function);
        if params.len() != args.len() {
            return Err(Error::at(
                call.span(),
                format!(
                    "`{class}::{method}` takes {} arguments but {} were passed",
                    params.len(),
                    args.len()
                ),
            ));
        }
        for ((pattern, ty), argument) in params.iter().zip(args.iter()) {
            bindings.push(self.binding(pattern, ty, argument)?);
        }
        self.self_class = Some(class.to_owned());
        let result =
            self.inline_resolved(helper, Inline::Value, call.span(), bindings);
        self.self_class = None;
        result
    }
}

fn host_path(path: &str) -> Result<&'static str, Error> {
    let value = match path {
        "now_ms" => "noctalia.nowMs()",
        "dark" => "noctalia.isDarkMode()",
        "outputs" => "noctalia.outputs()",
        "focused_output_name" => "noctalia.focusedOutputName()",
        "pluginDir" => "noctalia.pluginDir()",
        "pluginDataDir" => "noctalia.pluginDataDir()",
        "clipboard" => "noctalia.clipboardText()",
        "calendar" => "noctalia.calendarEvents()",
        "media" => "noctalia.mediaPlayer()",
        "stats" => "noctalia.systemStats()",
        "nightlight.state" => "noctalia.nightlight.state",
        "compositor.name" => "noctalia.compositor.name",
        "bar.vertical" => "barWidget.isVertical()",
        "bar.output_name" => "barWidget.outputName()",
        _ => {
            return Err(Error::plain(format!(
                "host path `{path}` is not supported by the Luau compiler"
            )));
        }
    };
    Ok(value)
}

fn is_iterator_adaptor(expr: &Expr) -> bool {
    let Expr::MethodCall(call) = strip_reference(expr) else {
        return false;
    };
    matches!(
        call.method.to_string().as_str(),
        "map"
            | "filter"
            | "filter_map"
            | "take"
            | "skip"
            | "flatten"
            | "rsplit"
    )
}

/// `flatten` opens an inner loop, so a `break` in the callback would only
/// leave that inner loop.
fn has_flatten_adaptor(expr: &Expr) -> bool {
    let Expr::MethodCall(call) = strip_reference(expr) else {
        return false;
    };
    call.method == "flatten" || has_flatten_adaptor(&call.receiver)
}

/// Every character an or-pattern of literals matches.
fn pattern_chars(pattern: &Pat, out: &mut String) -> Result<bool, Error> {
    match pattern {
        Pat::Or(or) => {
            for case in &or.cases {
                if !pattern_chars(case, out)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        Pat::Paren(paren) => pattern_chars(&paren.pat, out),
        Pat::Lit(lit) => match constant(&Expr::Lit(lit.clone())) {
            Some((text, _)) => {
                out.push_str(&unquote(&text));
                Ok(true)
            }
            None => Ok(false),
        },
        Pat::Path(path) => match path.path.segments.last() {
            Some(segment) => match constant(&Expr::Path(syn::ExprPath {
                attrs: Vec::new(),
                qself: None,
                path: syn::Path {
                    leading_colon: None,
                    segments: std::iter::once(segment.clone()).collect(),
                },
            })) {
                Some((text, _)) => {
                    out.push_str(&unquote(&text));
                    Ok(true)
                }
                None => Ok(false),
            },
            None => Ok(false),
        },
        _ => Ok(false),
    }
}
