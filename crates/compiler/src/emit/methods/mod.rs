//! Method-call lowering, split by receiver class.
//!
//! `mod.rs` classifies a call and holds the dispatch shared by every class:
//! context methods, statics, crate methods, `Option`/`Result` combinators,
//! iterators and the `entry` chain. `value.rs` covers the `serde_json` value
//! model, `string.rs` string methods, `number.rs` numeric methods,
//! `collection.rs` `Vec`/`HashMap` and `Option`/`Result` values, and `fs.rs`
//! and `path.rs` are the `std::fs` and `std::path` receivers.

use super::*;

mod collection;
mod fs;
mod number;
mod path;
mod string;
mod value;

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
            "map_err" => {
                return Ok(Some((receiver.to_owned(), ty)));
            }
            "get_or_init" | "get_or_try_init"
                if is_identifier(receiver)
                    || (receiver.starts_with(|c: char| {
                        c.is_ascii_alphabetic() || c == '_'
                    }) && (receiver.ends_with(']')
                        || receiver.contains('.'))) =>
            {
                let closure = arg(args, 0, call)?;
                self.line(&format!("if {receiver} == nil then"));
                self.indent = self.indent.saturating_add(1);
                let (text, value_ty) = self.invoke_arg(closure, &[])?;
                if text.is_empty() || value_ty.base == Base::Void {
                    self.line(&format!("{receiver} = true"));
                } else {
                    self.line(&format!("{receiver} = {text}"));
                }
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                return Ok(Some((
                    receiver.to_owned(),
                    Ty {
                        result: false,
                        ..value_ty
                    },
                )));
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
        if method != "nth"
            && !matches!(
                method.as_str(),
                "any"
                    | "all"
                    | "sum"
                    | "count"
                    | "for_each"
                    | "find"
                    | "position"
            )
        {
            return Ok(None);
        }
        let args = call.args.iter().collect::<Vec<_>>();
        if method == "nth" {
            let wanted = self.expr(arg(&args, 0, call)?)?.0;
            let result = self.temp();
            let seen = self.temp();
            self.line(&format!("local {result}"));
            self.line(&format!("local {seen} = 0"));
            let body = |emitter: &mut Self, value: &str| -> Result<(), Error> {
                emitter.line(&format!("{seen} = {seen} + 1"));
                emitter.line(&format!("if {seen} == ({wanted}) then"));
                emitter.indent = emitter.indent.saturating_add(1);
                emitter.line(&format!("{result} = {value}"));
                emitter.line("break");
                emitter.indent = emitter.indent.saturating_sub(1);
                emitter.line("end");
                Ok(())
            };
            self.emit_iter(&call.receiver, &body)?;
            return Ok(Some((result, Ty::OPT_VALUE)));
        }
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
            let result = self.temp();
            self.line(&format!("local {result}"));
            let index = (method == "position").then(|| {
                let index = self.temp();
                self.line(&format!("local {index} = 0"));
                index
            });
            let nested = has_flatten_adaptor(&call.receiver);
            let body = |emitter: &mut Self, value: &str| -> Result<(), Error> {
                if let Some(index) = &index {
                    emitter.line(&format!("{index} = {index} + 1"));
                }
                let (text, _) = emitter.invoke_arg(
                    closure,
                    std::slice::from_ref(&value.to_owned()),
                )?;
                let condition = if nested {
                    format!("{result} == nil and ({text})")
                } else {
                    text.clone()
                };
                emitter.line(&format!("if {condition} then"));
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
                "splitn" | "match_indices" => {
                    let args = call.args.iter().collect::<Vec<_>>();
                    let (text, ty) = self.expr(&call.receiver)?;
                    let (list, _) = self
                        .string_method(
                            call,
                            &call.method.to_string(),
                            &args,
                            &text,
                            ty,
                        )?
                        .ok_or_else(|| {
                            Error::at(
                                call.method.span(),
                                "iterator method is not supported",
                            )
                        })?;
                    return self.emit_table(&list, false, false, body);
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
                "char_indices" => {
                    let (text, _) = self.expr(&call.receiver)?;
                    let pairs = format!(
                        "(function() local __out = {{}}; for __offset, __code in utf8.codes({text}) do __out[#__out + 1] = {{ __offset - 1, utf8.char(__code) }} end; __out.__next = 1; return __out end)()"
                    );
                    return self.emit_table(&pairs, false, false, body);
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
                "rev" => {
                    let (source, _) = self.expr(&call.receiver)?;
                    let table = self.temp();
                    let index = self.temp();
                    self.line(&format!("local {table} = {source}"));
                    self.line(&format!("for {index} = #{table}, 1, -1 do"));
                    self.indent = self.indent.saturating_add(1);
                    let result = body(self, &format!("{table}[{index}]"));
                    self.indent = self.indent.saturating_sub(1);
                    self.line("end");
                    return result;
                }
                "enumerate" => {
                    let index = self.temp();
                    self.line(&format!("local {index} = 0"));
                    let inner = |emitter: &mut Self,
                                 value: &str|
                     -> Result<(), Error> {
                        emitter.line(&format!("{index} = {index} + 1"));
                        body(emitter, &format!("{{ {index}, {value} }}"))
                    };
                    return self.emit_iter(&call.receiver, &inner);
                }
                "zip" => {
                    let args = call.args.iter().collect::<Vec<_>>();
                    let (other, _) = self.expr(arg(&args, 0, call)?)?;
                    let other_table = self.temp();
                    let index = self.temp();
                    self.line(&format!("local {other_table} = {other}"));
                    self.line(&format!("local {index} = 0"));
                    let inner = |emitter: &mut Self,
                                 value: &str|
                     -> Result<(), Error> {
                        emitter.line(&format!("{index} = {index} + 1"));
                        body(
                            emitter,
                            &format!("{{ {value}, {other_table}[{index}] }}"),
                        )
                    };
                    return self.emit_iter(&call.receiver, &inner);
                }
                "take_while" | "skip_while" => {
                    let args = call.args.iter().collect::<Vec<_>>();
                    let closure = arg(&args, 0, call)?;
                    let active = self.temp();
                    self.line(&format!("local {active} = true"));
                    let taking = call.method == "take_while";
                    let inner = |emitter: &mut Self,
                                 value: &str|
                     -> Result<(), Error> {
                        emitter.line(&format!("if {active} then"));
                        emitter.indent = emitter.indent.saturating_add(1);
                        let (condition, _) = emitter.invoke_arg(
                            closure,
                            std::slice::from_ref(&value.to_owned()),
                        )?;
                        if taking {
                            emitter.line(&format!(
                                "if not ({condition}) then {active} = false end"
                            ));
                            emitter.line(&format!("if {active} then"));
                            emitter.indent = emitter.indent.saturating_add(1);
                            body(emitter, value)?;
                            emitter.indent = emitter.indent.saturating_sub(1);
                            emitter.line("end");
                        } else {
                            emitter.line(&format!(
                                "if {condition} then {active} = false end"
                            ));
                            emitter.line(&format!("if not {active} then"));
                            emitter.indent = emitter.indent.saturating_add(1);
                            body(emitter, value)?;
                            emitter.indent = emitter.indent.saturating_sub(1);
                            emitter.line("end");
                        }
                        emitter.indent = emitter.indent.saturating_sub(1);
                        emitter.line("end");
                        Ok(())
                    };
                    return self.emit_iter(&call.receiver, &inner);
                }
                "flat_map" => {
                    let args = call.args.iter().collect::<Vec<_>>();
                    let closure = arg(&args, 0, call)?;
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
                        emitter.emit_table(
                            &format!("({text})"),
                            false,
                            false,
                            body,
                        )
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
                    if matches!(ty.base, Base::List | Base::Value | Base::Ops) {
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
            Expr::Range(range) => {
                let list = self.range_list(range)?;
                return self.emit_table(&list, false, false, body);
            }
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

    /// `map.entry(key).or_insert(value)` reads and writes the slot under one
    /// evaluation of the map and the key.
    fn entry_method(
        &mut self,
        call: &ExprMethodCall,
        method: &str,
        args: &[&Expr],
    ) -> Result<OptionalValue, Error> {
        if !matches!(method, "or_insert" | "or_insert_with" | "or_default") {
            return Ok(None);
        }
        let mut modify = None;
        let mut node = strip_reference(&call.receiver);
        if let Expr::MethodCall(inner) = node
            && inner.method == "and_modify"
        {
            let inner_args = inner.args.iter().collect::<Vec<_>>();
            modify = Some(arg(&inner_args, 0, inner)?);
            node = strip_reference(&inner.receiver);
        }
        let Expr::MethodCall(entry) = node else {
            return Ok(None);
        };
        if entry.method != "entry" {
            return Ok(None);
        }
        let entry_args = entry.args.iter().collect::<Vec<_>>();
        let key = arg(&entry_args, 0, entry)?;
        let (receiver, _) = self.expr(&entry.receiver)?;
        let (key, _) = self.expr(key)?;
        let map = self.temp();
        let slot = self.temp();
        self.line(&format!("local {map} = {receiver}"));
        self.line(&format!("local {slot} = {key}"));
        let value = self.temp();
        self.line(&format!("local {value} = {map}[{slot}]"));
        if let Some(modify) = modify {
            self.line(&format!("if {value} ~= nil then"));
            self.indent = self.indent.saturating_add(1);
            let (text, _) =
                self.invoke_arg(modify, std::slice::from_ref(&value))?;
            self.line(&format!("{value} = {text}"));
            self.line(&format!("{map}[{slot}] = {value}"));
            self.indent = self.indent.saturating_sub(1);
            self.line("end");
        }
        let (default, ty) = match method {
            "or_insert" => self.expr(arg(args, 0, call)?)?,
            "or_insert_with" => self.invoke_arg(arg(args, 0, call)?, &[])?,
            _ => {
                return Err(Error::at(
                    call.method.span(),
                    "entry().or_default() needs the default of the map value, which the Luau model cannot derive; insert the default explicitly",
                ));
            }
        };
        self.line(&format!("if {value} == nil then"));
        self.indent = self.indent.saturating_add(1);
        self.line(&format!("{value} = {default}"));
        self.line(&format!("{map}[{slot}] = {value}"));
        self.indent = self.indent.saturating_sub(1);
        self.line("end");
        Ok(Some((
            value,
            Ty {
                optional: false,
                ..ty
            },
        )))
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
        if let Some(value) = self.entry_method(call, &method, &args)? {
            return Ok(value);
        }
        if method == "contains"
            && let Expr::Range(range) = strip_reference(&call.receiver)
        {
            return self.range_contains(call, &args, range);
        }
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
        if let Some((text, ty)) =
            self.value_method(call, &method, &args, &receiver, ty)?
        {
            return Ok((text, ty));
        }
        if let Some((text, ty)) =
            self.number_method(call, &method, &args, &receiver, ty)?
        {
            return Ok((text, ty));
        }
        if let Some((text, ty)) =
            self.string_method(call, &method, &args, &receiver, ty)?
        {
            return Ok((text, ty));
        }
        if let Some((text, ty)) =
            self.collection_method(call, &method, &args, &receiver, ty)?
        {
            return Ok((text, ty));
        }
        if let Some((text, ty)) =
            self.fs_method(call, &method, &args, &receiver, ty)?
        {
            return Ok((text, ty));
        }
        if let Some((text, ty)) =
            self.path_method(call, &method, &args, &receiver, ty)?
        {
            return Ok((text, ty));
        }
        Err(Error::at(
            call.method.span(),
            format!("method `{method}` is not supported by the Luau compiler"),
        ))
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
                let path = self.string_literal_arg(args, 0, call)?;
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

    fn class_name_of(&self, expr: &Expr) -> Option<String> {
        if let Some(class) = self.class_of_receiver(expr) {
            return Some(class);
        }
        let Expr::Call(call) = strip_reference(expr) else {
            return None;
        };
        let Expr::Path(path) = &*call.func else {
            return None;
        };
        let segments = path_segments(&path.path)?;
        let resolved = self.program.resolve(&self.module, &segments)?;
        match &resolved.function.sig.output {
            syn::ReturnType::Type(_, ty) => Some(type_text(ty)),
            syn::ReturnType::Default => None,
        }
        .filter(|name| !name.is_empty())
    }

    fn known_string_ty(&self, expr: &Expr) -> Option<Ty> {
        let expr = strip_reference(expr);
        match expr {
            Expr::Field(field) => {
                let Member::Named(member) = &field.member else {
                    return None;
                };
                let class = self.class_name_of(&field.base)?;
                let def = self.program.struct_def(&self.module, &class)?;
                let (_, ty) = def
                    .fields
                    .iter()
                    .find(|(name, _)| name == &member.to_string())?;
                (type_hint(ty).base == Base::Str).then_some(Ty::STR)
            }
            Expr::Call(call) => {
                let Expr::Path(path) = &*call.func else {
                    return None;
                };
                let segments = path_segments(&path.path)?;
                let resolved = self.program.resolve(&self.module, &segments)?;
                match &resolved.function.sig.output {
                    syn::ReturnType::Type(_, ty) => {
                        (type_hint(ty).base == Base::Str).then_some(Ty::STR)
                    }
                    syn::ReturnType::Default => None,
                }
            }
            _ => None,
        }
    }
    fn string_arg(
        &mut self,
        args: &[&Expr],
        index: usize,
        call: &ExprMethodCall,
    ) -> Result<String, Error> {
        let value = arg(args, index, call)?;
        let (text, ty) = self.expr(value)?;
        if ty.base != Base::Str && self.known_string_ty(value).is_none() {
            return Err(Error::at(
                value.span(),
                "expected a string argument with a known type",
            ));
        }
        Ok(text)
    }
    fn string_literal_arg(
        &mut self,
        args: &[&Expr],
        index: usize,
        call: &ExprMethodCall,
    ) -> Result<String, Error> {
        let value = arg(args, index, call)?;
        let text = self.string_arg(args, index, call)?;
        if !text.starts_with('"') {
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

/// `i64` bounds as Luau literals. `u64::MAX` rounds to 2^64, which keeps the
/// comparison below exact because no integer above `i64::MAX` is representable
/// as a Luau integer anyway.
pub(super) const I64_RANGE: (&str, &str) =
    ("-9223372036854775808", "9223372036854775807");
pub(super) const U64_RANGE: (&str, &str) = ("0", "18446744073709551615");
pub(super) const I128_RANGE: (&str, &str) =
    ("-1.7014118346046923e38", "1.7014118346046923e38");

/// `Value::as_str` and friends only answer for the matching JSON type. A
/// receiver already known to hold that type needs no check.
fn string_check(receiver: &str, ty: Ty) -> String {
    if ty.base == Base::Str {
        receiver.to_owned()
    } else {
        format!("(type({receiver}) == \"string\" and {receiver} or nil)")
    }
}

fn boolean_check(receiver: &str, ty: Ty) -> String {
    if ty.base == Base::Bool {
        receiver.to_owned()
    } else {
        format!("(type({receiver}) == \"boolean\" and {receiver} or nil)")
    }
}

fn number_check(receiver: &str, ty: Ty) -> String {
    if ty.is_numeric() {
        receiver.to_owned()
    } else {
        format!("(type({receiver}) == \"number\" and {receiver} or nil)")
    }
}

/// `serde_json` stores integers and floats apart, and Luau keeps the same
/// distinction in its integer subtype. A value that is integral and inside the
/// bounds is an integer whatever its storage.
fn integer_test(receiver: &str, ty: Ty, range: (&str, &str)) -> String {
    let (low, high) = range;
    match ty.base {
        Base::Int => "(true)".to_owned(),
        // `serde_json` keeps integers and floats apart, and a value that a
        // float expression produced is a float however it prints.
        Base::Float => "(false)".to_owned(),
        _ => format!(
            "(type({receiver}) == \"number\" and {receiver} % 1 == 0 and {receiver} >= {low} and {receiver} <= {high})"
        ),
    }
}

fn integer_check(receiver: &str, ty: Ty, range: (&str, &str)) -> String {
    match ty.base {
        Base::Int => receiver.to_owned(),
        Base::Float => "nil".to_owned(),
        _ => format!(
            "({} and {receiver} or nil)",
            integer_test(receiver, ty, range)
        ),
    }
}

/// A JSON null has no representation inside a Luau container: writing nil
/// removes the slot, so a computed `Value::Null` would silently disappear.
pub(super) fn reject_null(
    text: &str,
    span: proc_macro2::Span,
    construct: &str,
) -> Result<(), Error> {
    if text != "nil" {
        return Ok(());
    }
    Err(Error::at(
        span,
        format!(
            "json null cannot be stored by {construct}: a Luau table has no null, so the key or slot would disappear; omit the key or guard the value with is_null()"
        ),
    ))
}

/// `Value::as_object` as a Luau expression. A list is never an object and a
/// `Map` always is, so only an unknown receiver needs the runtime shape test.
fn object_test(receiver: &str, ty: Ty, uses_array_shape: &mut bool) -> String {
    match ty.base {
        Base::List => "nil".to_owned(),
        Base::Table => receiver.to_owned(),
        _ => {
            *uses_array_shape = true;
            format!(
                "(type({receiver}) == \"table\" and not is_array({receiver}) and {receiver} or nil)"
            )
        }
    }
}

/// The assignable expression behind lock and deref wrappers, so `take()` can
/// clear the storage the receiver reads from.
fn guard_target(expr: &Expr) -> &Expr {
    let expr = strip_reference(expr);
    let Expr::MethodCall(call) = expr else {
        return expr;
    };
    if !matches!(
        call.method.to_string().as_str(),
        "lock"
            | "read"
            | "write"
            | "borrow"
            | "borrow_mut"
            | "deref"
            | "as_mut"
            | "as_ref"
            | "unwrap"
            | "expect"
            | "unwrap_or_else"
    ) {
        return expr;
    }
    guard_target(&call.receiver)
}

/// `flatten` opens an inner loop, so a `break` in the callback would only
/// leave that inner loop.
fn has_flatten_adaptor(expr: &Expr) -> bool {
    let Expr::MethodCall(call) = strip_reference(expr) else {
        return false;
    };
    matches!(call.method.to_string().as_str(), "flatten" | "flat_map")
        || has_flatten_adaptor(&call.receiver)
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
