use super::*;

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
                Ok((format!("math.{method}({receiver}, {other})"), Ty::FLOAT))
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
                if matches!(ty.base, Base::Int | Base::Value) =>
            {
                let other = self.expr(arg(args, 0, call)?)?.0;
                let operator = match method {
                    "saturating_add" => "+",
                    "saturating_sub" => "-",
                    _ => "*",
                };
                Ok((
                    format!(
                        "math.clamp((math.floor({receiver}) {operator} math.floor({other})), (-9223372036854775808), 9223372036854775807)"
                    ),
                    ty,
                ))
            }
            "wrapping_add" | "wrapping_sub"
                if matches!(ty.base, Base::Int | Base::Value) =>
            {
                let other = self.expr(arg(args, 0, call)?)?.0;
                let operator = if method == "wrapping_add" { "+" } else { "-" };
                Ok((
                    format!(
                        "(math.floor({receiver}) {operator} math.floor({other}))"
                    ),
                    ty,
                ))
            }
            "div_ceil" if matches!(ty.base, Base::Int | Base::Value) => {
                let other = self.expr(arg(args, 0, call)?)?.0;
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
                Ok((
                    format!(
                        "(function() local __v = {receiver}; if __v == math.huge then return true end; if __v == -math.huge then return false end; return (1 / __v) > 0 end)()"
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
                Ok((
                    format!(
                        "(function() local __a, __b = {receiver}, {other}; if __a ~= __a then if __b ~= __b then return \"Equal\" end return \"Less\" end; if __b ~= __b then return \"Greater\" end; if __a < __b then return \"Less\" end; if __a > __b then return \"Greater\" end; if __a == 0 and __b == 0 then if (1 / __a) < (1 / __b) then return \"Less\" end; if (1 / __a) > (1 / __b) then return \"Greater\" end end; return \"Equal\" end)()"
                    ),
                    Ty::VALUE,
                ))
            }
            _ => return Ok(None),
        };
        Ok(Some(value?))
    }
}
