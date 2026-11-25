use super::*;

impl<'a> Emitter<'a> {
    pub(super) fn value_method(
        &mut self,
        call: &ExprMethodCall,
        method: &str,
        args: &[&Expr],
        receiver: &str,
        ty: Ty,
    ) -> Result<OptionalValue, Error> {
        let value: Result<(String, Ty), Error> = match method {
            "to_string"
                if matches!(ty.base, Base::Value | Base::Int | Base::Float) =>
            {
                Ok((format!("json.encode({receiver})"), Ty::STR))
            }
            "as_object_mut" => {
                let test =
                    object_test(receiver, ty, &mut self.uses_array_shape);
                Ok((test, Ty::OPT_VALUE))
            }
            "entry" => Err(Error::at(
                call.method.span(),
                "the entry API is only supported as entry(key).or_insert(value), entry(key).or_insert_with(f) or entry(key).and_modify(f).or_insert(value)",
            )),
            "as_str" => Ok((string_check(receiver, ty), Ty::OPT_STR)),
            "as_i64" => {
                Ok((integer_check(receiver, ty, I64_RANGE), Ty::OPT_INT))
            }
            "as_u64" => {
                Ok((integer_check(receiver, ty, U64_RANGE), Ty::OPT_INT))
            }
            "as_f64" | "as_number" => {
                Ok((number_check(receiver, ty), Ty::OPT_FLOAT))
            }
            "as_bool" => Ok((boolean_check(receiver, ty), Ty::OPT_BOOL)),
            "as_array" | "as_array_mut" if ty.base == Base::List => {
                Ok((receiver.to_owned(), Ty::OPT_LIST))
            }
            "as_array" | "as_array_mut" => {
                self.uses_array_shape = true;
                Ok((
                    format!("(is_array({receiver}) and {receiver} or nil)"),
                    Ty::OPT_LIST,
                ))
            }
            "is_null" => Ok((format!("({receiver} == nil)"), Ty::BOOL)),
            "as_object" => {
                let test =
                    object_test(receiver, ty, &mut self.uses_array_shape);
                Ok((test, Ty::OPT_VALUE))
            }
            "is_array" => match ty.base {
                Base::List => Ok(("true".to_owned(), Ty::BOOL)),
                Base::Table => Ok(("false".to_owned(), Ty::BOOL)),
                _ => {
                    self.uses_array_shape = true;
                    Ok((format!("is_array({receiver})"), Ty::BOOL))
                }
            },
            "is_object" => match ty.base {
                Base::List => Ok(("false".to_owned(), Ty::BOOL)),
                Base::Table => Ok(("true".to_owned(), Ty::BOOL)),
                _ => {
                    self.uses_array_shape = true;
                    Ok((
                        format!(
                            "(type({receiver}) == \"table\" and not is_array({receiver}))"
                        ),
                        Ty::BOOL,
                    ))
                }
            },
            "is_string" => {
                Ok((format!("(type({receiver}) == \"string\")"), Ty::BOOL))
            }
            "is_number" => {
                Ok((format!("(type({receiver}) == \"number\")"), Ty::BOOL))
            }
            "is_i64" => Ok((integer_test(receiver, ty, I64_RANGE), Ty::BOOL)),
            "is_u64" => Ok((integer_test(receiver, ty, U64_RANGE), Ty::BOOL)),
            "is_f64" | "is_f32" => {
                Ok((format!("(type({receiver}) == \"number\")"), Ty::BOOL))
            }
            "is_i128" | "is_u128" => {
                Ok((integer_test(receiver, ty, I128_RANGE), Ty::BOOL))
            }
            "is_boolean" => {
                Ok((format!("(type({receiver}) == \"boolean\")"), Ty::BOOL))
            }
            "len" if matches!(ty.base, Base::Value | Base::Table) => {
                self.uses_count_keys = true;
                Ok((format!("count_keys({receiver})"), Ty::INT))
            }
            "contains_key" => {
                let (key, key_ty) = self.expr(arg(args, 0, call)?)?;
                if ty.base != Base::Table && key_ty.base == Base::Int {
                    Ok((format!("({receiver}[({key}) + 1] ~= nil)"), Ty::BOOL))
                } else {
                    Ok((format!("({receiver}[{key}] ~= nil)"), Ty::BOOL))
                }
            }
            "take" if args.is_empty() => {
                let target = guard_target(&call.receiver);
                if !matches!(
                    target,
                    Expr::Path(_) | Expr::Field(_) | Expr::Index(_)
                ) {
                    return Err(Error::at(
                        call.method.span(),
                        "take() needs a variable, field or index to clear",
                    ));
                }
                let value = self.temp();
                self.line(&format!("local {value} = {receiver}"));
                let mut is_local = false;
                let target = self.assign_target(target, &mut is_local)?;
                self.line(&format!("{target} = nil"));
                Ok((value, Ty::new(ty.base)))
            }
            "keys" => {
                let keys = self.temp();
                self.line(&format!(
                    "local {keys} = (function() local __keys = {{}} for __key in pairs({receiver}) do __keys[#__keys + 1] = __key end table.sort(__keys, function(a, b) return tostring(a) < tostring(b) end) __keys.__next = 1 return __keys end)()"
                ));
                Ok((keys, Ty::LIST))
            }
            _ => return Ok(None),
        };
        Ok(Some(value?))
    }
}
