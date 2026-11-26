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
            _ => return Ok(None),
        };
        Ok(Some(value?))
    }
}
