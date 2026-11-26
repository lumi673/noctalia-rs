use super::*;

fn int_literal(expr: &Expr) -> Option<(i128, usize, bool)> {
    match strip_reference(expr) {
        Expr::Lit(ExprLit {
            lit: Lit::Int(value),
            ..
        }) => {
            let suffix = value.suffix();
            let (bits, signed) = match suffix {
                "i8" => (8, true),
                "i16" => (16, true),
                "i32" => (32, true),
                "u8" => (8, false),
                "u16" => (16, false),
                "u32" => (32, false),
                "u64" | "usize" => (64, false),
                "i64" | "isize" | "" => (64, true),
                _ => return None,
            };
            Some((value.base10_parse::<i128>().ok()?, bits, signed))
        }
        Expr::Binary(binary) => {
            let left = int_literal(&binary.left)?;
            let right = int_literal(&binary.right)?;
            let value = match binary.op {
                syn::BinOp::Add(_) => left.0.checked_add(right.0)?,
                syn::BinOp::Sub(_) => left.0.checked_sub(right.0)?,
                _ => return None,
            };
            Some((value, left.1, left.2))
        }
        other => {
            let (text, _) = constant(other)?;
            let value = text.trim_matches(['(', ')']).parse::<i128>().ok()?;
            let signed = !matches!(other, Expr::Path(path) if path.path.segments.iter().any(|segment| segment.ident.to_string().starts_with('u')));
            Some((value, 64, signed))
        }
    }
}

fn int_bounds(bits: usize, signed: bool) -> (i128, i128) {
    if signed {
        let max = (1i128 << (bits - 1)) - 1;
        (-max - 1, max)
    } else {
        (0, (1i128 << bits) - 1)
    }
}
impl<'a> Emitter<'a> {
    pub(super) fn number_method(
        &mut self,
        call: &ExprMethodCall,
        method: &str,
        args: &[&Expr],
        receiver: &str,
        ty: Ty,
    ) -> Result<OptionalValue, Error> {
        let value: Result<(String, Ty), Error> = match method {
            "is_nan" => Ok((format!("({receiver} ~= {receiver})"), Ty::BOOL)),
            "div_euclid" => {
                let other = self.expr(arg(args, 0, call)?)?.0;
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
            "trunc" => Ok((
                format!(
                    "(({receiver} >= 0) and math.floor({receiver}) or math.ceil({receiver}))"
                ),
                Ty::FLOAT,
            )),
            "round" => Ok((format!("math.round({receiver})"), Ty::FLOAT)),
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
                let min = self.expr(arg(args, 0, call)?)?.0;
                let max = self.expr(arg(args, 1, call)?)?.0;
                Ok((format!("math.clamp({receiver}, {min}, {max})"), Ty::FLOAT))
            }
            "max" | "min" => {
                let other = self.expr(arg(args, 0, call)?)?.0;
                if ty.base == Base::Int {
                    Ok((format!("math.{method}({receiver}, {other})"), ty))
                } else {
                    Ok((
                        format!(
                            "(function() local __a, __b = {receiver}, {other}; if __a ~= __a then return __b end; if __b ~= __b then return __a end; return math.{method}(__a, __b) end)()"
                        ),
                        ty,
                    ))
                }
            }
            "rem_euclid" => {
                let other = self.expr(arg(args, 0, call)?)?.0;
                Ok((
                    format!("(({receiver} % {other}) + {other}) % {other}"),
                    Ty::FLOAT,
                ))
            }
            "duration_since" | "as_millis" => {
                Ok((receiver.to_owned(), Ty::INT))
            }
            "as_secs" => {
                Ok((format!("math.floor({receiver} / 1000)"), Ty::INT))
            }
            "as_nanos" => Ok((format!("({receiver} * 1000000)"), Ty::INT)),
            "saturating_add" | "saturating_sub" | "saturating_mul"
            | "wrapping_add" | "wrapping_sub" => {
                let argument = arg(args, 0, call)?;
                let Some((left, bits, signed)) = int_literal(&call.receiver)
                else {
                    return Err(Error::at(
                        call.method.span(),
                        "integer edge methods need exact compile-time operands",
                    ));
                };
                let Some((right, _, _)) = int_literal(argument) else {
                    return Err(Error::at(
                        call.method.span(),
                        "integer edge methods need exact compile-time operands",
                    ));
                };
                let (min, max) = int_bounds(bits, signed);
                let raw = match method {
                    "saturating_add" | "wrapping_add" => left + right,
                    "saturating_sub" | "wrapping_sub" => left - right,
                    _ => left * right,
                };
                let result = if method.starts_with("wrapping_") {
                    let span = max - min + 1;
                    ((raw - min).rem_euclid(span)) + min
                } else {
                    raw.clamp(min, max)
                };
                Ok((format!("({result})"), ty))
            }
            "div_ceil" if matches!(ty.base, Base::Int | Base::Value) => {
                let other_expr = arg(args, 0, call)?;
                if let (Some((left, _, _)), Some((right, _, _))) =
                    (int_literal(&call.receiver), int_literal(other_expr))
                    && left == i64::MIN as i128
                    && right == -1
                {
                    return Err(Error::at(
                        call.method.span(),
                        "integer division overflows",
                    ));
                }
                let other = self.expr(other_expr)?.0;
                Ok((
                    format!(
                        "(function() local __a, __b = math.floor({receiver}), math.floor({other}); if __b == 0 then error(\"divide by zero\") end; local __q = (__a >= 0 and math.floor(__a / __b) or math.ceil(__a / __b)); local __r = __a - __q * __b; if __r ~= 0 and ((__r > 0) == (__b > 0)) then return __q + 1 end; return __q end)()"
                    ),
                    ty,
                ))
            }
            "is_sign_positive"
                if matches!(ty.base, Base::Float | Base::Value) =>
            {
                let nan_result = if matches!(
                    strip_reference(&call.receiver),
                    Expr::MethodCall(inner) if inner.method == "abs"
                ) {
                    "true"
                } else {
                    "error(\"NaN sign is not observable in Luau\")"
                };
                Ok((
                    format!(
                        "(function() local __v = {receiver}; if __v ~= __v then return {nan_result} end; if __v == math.huge then return true end; if __v == -math.huge then return false end; return (1 / __v) > 0 end)()"
                    ),
                    Ty::BOOL,
                ))
            }
            "powi" => {
                let exponent = self.expr(arg(args, 0, call)?)?.0;
                Ok((format!("math.pow({receiver}, {exponent})"), Ty::FLOAT))
            }
            "log10" => Ok((format!("math.log({receiver}, 10)"), Ty::FLOAT)),
            "cmp" if ty.base == Base::Int => {
                let other = self.expr(arg(args, 0, call)?)?.0;
                Ok((
                    format!(
                        "(function() local __a, __b = math.floor({receiver}), math.floor({other}); if __a < __b then return \"Less\" end; if __a > __b then return \"Greater\" end; return \"Equal\" end)()"
                    ),
                    Ty::VALUE,
                ))
            }
            "partial_cmp" if matches!(ty.base, Base::Float | Base::Value) => {
                let other = self.expr(arg(args, 0, call)?)?.0;
                Ok((
                    format!(
                        "(function() local __a, __b = {receiver}, {other}; if __a ~= __a or __b ~= __b then return nil end; if __a < __b then return \"Less\" end; if __a > __b then return \"Greater\" end; return \"Equal\" end)()"
                    ),
                    Ty::OPT_VALUE,
                ))
            }
            "total_cmp" if ty.base == Base::Float => {
                let other = self.expr(arg(args, 0, call)?)?.0;
                let nan_greater = matches!(
                    strip_reference(&call.receiver),
                    Expr::MethodCall(inner) if inner.method == "abs"
                );
                let a_nan = if nan_greater {
                    "\"Greater\""
                } else {
                    "error(\"NaN total order needs a sign bit\")"
                };
                let b_nan = if nan_greater {
                    "\"Less\""
                } else {
                    "error(\"NaN total order needs a sign bit\")"
                };
                Ok((
                    format!(
                        "(function() local __a, __b = {receiver}, {other}; if __a ~= __a then if __b ~= __b then return \"Equal\" end return {a_nan} end; if __b ~= __b then return {b_nan} end; if __a < __b then return \"Less\" end; if __a > __b then return \"Greater\" end; if __a == 0 and __b == 0 then if (1 / __a) < (1 / __b) then return \"Less\" end; if (1 / __a) > (1 / __b) then return \"Greater\" end end; return \"Equal\" end)()"
                    ),
                    Ty::VALUE,
                ))
            }
            _ => return Ok(None),
        };
        Ok(Some(value?))
    }
}
