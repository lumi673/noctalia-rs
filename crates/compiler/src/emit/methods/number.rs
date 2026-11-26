use super::*;

/// Checks that a pair of 32-bit limbs round-trips through Luau's number model.
/// Powers of two above 2^53 are exact even though most neighbours are not.
const LIMB_EXACT: &str = concat!(
    "local function __bits(__v) local __n = 0 while __v >= 1 do __v = math.floor(__v / 2); __n = __n + 1 end return __n - 1 end;",
    "local function __exact(__h, __l) local __e = (__h > 0) and (32 + __bits(__h)) or __bits(__l); if __e <= 52 then return true end; local __shift = __e - 52; if __shift <= 32 then return (__l % (2 ^ __shift)) == 0 end; return __l == 0 and (__h % (2 ^ (__shift - 32))) == 0 end;",
);

/// Rejects a limb value that would change on conversion to a Luau number.
fn exact_guard(
    location: &str,
    construct: &str,
    high: &str,
    low: &str,
) -> String {
    let location = location.replace('"', "\\\"");
    let construct = construct.replace('"', "\\\"");
    format!(
        "if not __exact({high}, {low}) then error(\"{location}: {construct} is not exactly representable in Luau (1 construct)\") end;",
    )
}

fn exact_i128(value: i128) -> bool {
    value as f64 as i128 == value
}

/// Luau-only 64-bit magnitude arithmetic. Values are sign plus two 32-bit
/// limbs, so carries and borrows never pass through a double. The result is
/// converted to the Luau number model only after the operation completes.
fn exact_binary(
    method: &str,
    left: &str,
    right: &str,
    right_literal: Option<i128>,
    bits: usize,
    signed: bool,
    location: &str,
) -> String {
    let max = if signed {
        (1u128 << (bits - 1)) - 1
    } else {
        (1u128 << bits) - 1
    };
    let min_magnitude = if signed { max + 1 } else { 0 };
    let modulus = 1u128 << bits;
    let limbs = |value: u128| (value >> 32, value & 0xffff_ffff);
    let (max_hi, max_lo) = limbs(max);
    let (min_hi, min_lo) = limbs(min_magnitude);
    let (mod_hi, mod_lo) = limbs(modulus);
    let saturating = method.starts_with("saturating_");
    let right_sign = if method.ends_with("_sub") {
        "-__bs"
    } else {
        "__bs"
    };
    let operation = if method.ends_with("_mul") {
        concat!(
            "local __r4, __r3, __r2, __r1, __r0 = __mulmag(__ahi, __all, __bhi, __blo);",
            "local __hi, __lo = __r1, __r0;",
            "if __ahi == 0 and __all == 0 and __bhi == 0 and __blo == 0 then __s = 0 elseif __ahi == 0 and __all == 0 then __s = __bs elseif __bhi == 0 and __blo == 0 then __s = __as else __s = (__as == __bs) and 1 or -1 end;",
            "local __over = (__r4 ~= 0 or __r3 ~= 0 or __r2 ~= 0);",
        )
        .to_owned()
    } else {
        format!(
            concat!(
                "local __bs2 = {right_sign};",
                "if __ahi == 0 and __all == 0 and __bhi == 0 and __blo == 0 then __s = 0 elseif __ahi == 0 and __all == 0 then __s = __bs2 elseif __bhi == 0 and __blo == 0 then __s = __as elseif __as == __bs2 then __s = __as else __s = __cmp(__ahi, __all, __bhi, __blo) end;",
                "local __hi, __lo;",
                "if __as ~= 0 and __as == __bs2 then",
                "  __hi, __lo = __addmag(__ahi, __all, __bhi, __blo);",
                "elseif __cmp(__ahi, __all, __bhi, __blo) >= 0 then",
                "  __hi, __lo = __submag(__ahi, __all, __bhi, __blo);",
                "else",
                "  __hi, __lo = __submag(__bhi, __blo, __ahi, __all);",
                "end;",
                "local __over = false;",
            ),
            right_sign = right_sign
        )
    };
    let bound_check = if signed {
        format!(
            concat!(
                "if __s < 0 and __cmp(__hi, __lo, {min_hi}, {min_lo}) > 0 then __over = true end;",
                "if __s >= 0 and __cmp(__hi, __lo, {max_hi}, {max_lo}) > 0 then __over = true end;",
            ),
            min_hi = min_hi,
            min_lo = min_lo,
            max_hi = max_hi,
            max_lo = max_lo,
        )
    } else {
        format!(
            concat!(
                "if __s < 0 or __cmp(__hi, __lo, {max_hi}, {max_lo}) > 0 then __over = true end;",
            ),
            max_hi = max_hi,
            max_lo = max_lo,
        )
    };
    let finish = if saturating {
        if signed {
            format!(
                concat!(
                    "if __over and __s < 0 then __hi = {min_hi}; __lo = {min_lo}; __s = -1 end;",
                    "if __over and __s >= 0 then __hi = {max_hi}; __lo = {max_lo}; __s = 1 end;",
                    "local __result = __s * (__hi * __B + __lo);",
                ),
                min_hi = min_hi,
                min_lo = min_lo,
                max_hi = max_hi,
                max_lo = max_lo,
            )
        } else {
            format!(
                concat!(
                    "if __over and __s < 0 then __hi = 0; __lo = 0; __s = 0 end;",
                    "if __over and __s >= 0 then __hi = {max_hi}; __lo = {max_lo}; __s = 1 end;",
                    "local __result = __hi * __B + __lo;",
                ),
                max_hi = max_hi,
                max_lo = max_lo,
            )
        }
    } else {
        let small = mod_hi <= 1;
        let lo_mod = if mod_lo == 0 {
            "__B".to_owned()
        } else {
            mod_lo.to_string()
        };
        let mask = if small {
            format!("__hi = 0; __lo = __lo % {lo_mod};")
        } else {
            format!("__hi = __hi % {mod_hi};")
        };
        let sign = if signed {
            let complement = if small {
                format!(
                    "__hi = 0; if __lo ~= 0 then __lo = {lo_mod} - __lo end;"
                )
            } else {
                format!(
                    "__hi = ({mod_hi} - __hi - ((__lo == 0) and 0 or 1)) % __B; if __lo ~= 0 then __lo = {mod_lo} - __lo else __lo = 0 end;"
                )
            };
            format!(
                concat!(
                    "if __cmp(__hi, __lo, {min_hi}, {min_lo}) >= 0 then",
                    "  {complement}",
                    "  __s = -1;",
                    "end;",
                ),
                min_hi = min_hi,
                min_lo = min_lo,
                complement = complement,
            )
        } else {
            "__s = 1;".to_owned()
        };
        format!(
            concat!(
                "{mask}",
                "{sign}",
                "local __result = __s * (__hi * __B + __lo);",
            ),
            mask = mask,
            sign = sign,
        )
    };
    let guard =
        exact_guard(location, &format!("{method} result"), "__hi", "__lo");
    let operand_guard = format!(
        "{}{}",
        exact_guard(location, &format!("{method} operand"), "__ahi", "__all",),
        exact_guard(location, &format!("{method} operand"), "__bhi", "__blo",),
    );
    const LEFT_MARK: &str = "\u{e000}";
    const RIGHT_MARK: &str = "\u{e001}";
    let right_source = if right_literal.is_some() {
        String::new()
    } else {
        right.to_owned()
    };
    let right_operand = match right_literal {
        Some(value) => {
            let sign = if value < 0 { -1 } else { 1 };
            let magnitude = value.unsigned_abs();
            format!(
                "local __bs, __bhi, __blo = {sign}, {}, {};",
                magnitude >> 32,
                magnitude & 0xffff_ffff
            )
        }
        None => format!("local __bs, __bhi, __blo = __pack({RIGHT_MARK});"),
    };
    format!(
        concat!(
            "(function()",
            "local __B = 4294967296;",
            "{limb_exact}",
            "local function __pack(__v)",
            "  local __s = (__v < 0) and -1 or 1;",
            "  if __v < 0 then __s = -1; __v = -__v end;",
            "  local __m = math.floor(__v);",
            "  local __hi = math.floor(__m / __B) % __B;",
            "  local __lo = __m - math.floor(__m / __B) * __B;",
            "  return __s, __hi, __lo;",
            "end;",
            "local function __cmp(__ah, __al, __bh, __bl)",
            "  if __ah ~= __bh then return (__ah < __bh) and -1 or 1 end;",
            "  if __al ~= __bl then return (__al < __bl) and -1 or 1 end;",
            "  return 0;",
            "end;",
            "local function __addmag(__ah, __al, __bh, __bl)",
            "  local __l = __al + __bl;",
            "  local __c = math.floor(__l / __B);",
            "  return __ah + __bh + __c, __l - __c * __B;",
            "end;",
            "local function __submag(__ah, __al, __bh, __bl)",
            "  local __borrow = (__al < __bl) and 1 or 0;",
            "  return __ah - __bh - __borrow, __al - __bl + __borrow * __B;",
            "end;",
            "local function __mul32(__a, __b)",
            "  local __a0, __a1 = __a % 65536, math.floor(__a / 65536);",
            "  local __b0, __b1 = __b % 65536, math.floor(__b / 65536);",
            "  local __p0 = __a0 * __b0;",
            "  local __t = __a0 * __b1 + __a1 * __b0 + math.floor(__p0 / 65536);",
            "  return __a1 * __b1 + math.floor(__t / 65536), (__p0 % 65536) + (__t % 65536) * 65536;",
            "end;",
            "local function __mulmag(__ah, __al, __bh, __bl)",
            "  local __p0h, __p0l = __mul32(__al, __bl);",
            "  local __p1h, __p1l = __mul32(__al, __bh);",
            "  local __p2h, __p2l = __mul32(__ah, __bl);",
            "  local __p3h, __p3l = __mul32(__ah, __bh);",
            "  local __r0 = __p0l;",
            "  local __t = __p0h + __p1l + __p2l;",
            "  local __r1, __c = __t % __B, math.floor(__t / __B);",
            "  __t = __p1h + __p2h + __p3l + __c;",
            "  local __r2;",
            "  __r2, __c = __t % __B, math.floor(__t / __B);",
            "  __t = __p3h + __c;",
            "  return math.floor(__t / __B), __t % __B, __r2, __r1, __r0;",
            "end;",
            "local __as, __ahi, __all = __pack({left});",
            "{right_operand}",
            "{operand_guard}",
            "local __s = 0;",
            "{operation}",
            "{bound_check}",
            "{finish}",
            "{guard}",
            "return __result;",
            "end)()",
        ),
        left = LEFT_MARK,
        right_operand = right_operand,
        operation = operation,
        bound_check = bound_check,
        finish = finish,
        guard = guard,
        limb_exact = LIMB_EXACT,
        operand_guard = operand_guard,
    )
    .replace(';', "\n")
    .replace(LEFT_MARK, left)
    .replace(RIGHT_MARK, &right_source)
}

/// Decimal validation and limb conversion for `parse::<T>()`.
pub(super) fn exact_parse(
    receiver: &str,
    target: &str,
    location: &str,
) -> String {
    let (positive, negative, signed) = match target {
        "i8" => ("127", "128", true),
        "i16" => ("32767", "32768", true),
        "i32" => ("2147483647", "2147483648", true),
        "u8" => ("255", "0", false),
        "u16" => ("65535", "0", false),
        "u32" => ("4294967295", "0", false),
        "u64" | "usize" => ("18446744073709551615", "0", false),
        _ => ("9223372036854775807", "9223372036854775808", true),
    };
    const RECEIVER_MARK: &str = "\u{e002}";
    let guard = exact_guard(
        location,
        &format!("parse::<{target}> result"),
        "__hi",
        "__lo",
    );
    format!(
        concat!(
            "(function()",
            "local __s = {RECEIVER_MARK};",
            "local __start, __finish, __sign, __digits = string.find(__s, \"^([+-]?)(%d+)$\");",
            "if __start == nil then return nil, \"invalid digit found in string\" end;",
            "__digits = string.gsub(__digits, \"^0+\", \"\");",
            "if __digits == \"\" then __digits = \"0\" end;",
            "local __neg = __sign == \"-\";",
            "if __neg and not {signed} then return nil, \"number too large or small\" end;",
            "local __limit = __neg and \"{negative}\" or \"{positive}\";",
            "if #__digits > #__limit or (#__digits == #__limit and __digits > __limit) then return nil, \"number too large or small\" end;",
            "local __B = 4294967296;",
            "{limb_exact}",
            "local __hi, __lo = 0, 0;",
            "for __i = 1, #__digits do",
            "  local __v = __lo * 10 + (string.byte(__digits, __i) - 48);",
            "  local __c = math.floor(__v / __B);",
            "  __lo = __v - __c * __B;",
            "  __hi = (__hi * 10 + __c) % __B;",
            "end;",
            "if __hi == 0 and __lo == 0 then return 0 end;",
            "{guard}",
            "if __neg then return -(__hi * __B + __lo) end;",
            "return __hi * __B + __lo;",
            "end)()",
        ),
        RECEIVER_MARK = RECEIVER_MARK,
        signed = signed,
        negative = negative,
        positive = positive,
        guard = guard,
        limb_exact = LIMB_EXACT,
    )
    .replace(';', "\n")
    .replace(RECEIVER_MARK, receiver)
}

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
            let width = match other {
                Expr::Path(path) => {
                    path.path.segments.iter().rev().find_map(|segment| {
                        let name = segment.ident.to_string();
                        match name.as_str() {
                            "i8" | "u8" => Some((8, name.starts_with('i'))),
                            "i16" | "u16" => Some((16, name.starts_with('i'))),
                            "i32" | "u32" => Some((32, name.starts_with('i'))),
                            "i64" | "u64" | "isize" | "usize" => {
                                Some((64, name.starts_with('i')))
                            }
                            _ => None,
                        }
                    })
                }
                _ => None,
            };
            let (bits, signed) = width.unwrap_or((64, true));
            Some((value, bits, signed))
        }
    }
}

fn has_inexact_literal(expr: &Expr) -> bool {
    match strip_reference(expr) {
        Expr::Lit(ExprLit {
            lit: Lit::Int(_), ..
        }) => int_literal(expr).is_some_and(|(value, _, _)| !exact_i128(value)),
        Expr::Binary(binary) => {
            has_inexact_literal(&binary.left)
                || has_inexact_literal(&binary.right)
        }
        Expr::Paren(paren) => has_inexact_literal(&paren.expr),
        Expr::Group(group) => has_inexact_literal(&group.expr),
        Expr::Unary(unary) => has_inexact_literal(&unary.expr),
        Expr::Path(_) => {
            int_literal(expr).is_some_and(|(value, _, _)| !exact_i128(value))
        }
        _ => false,
    }
}
/// Exact `div_ceil` using 32-bit limbs. Double division cannot distinguish a
/// rounded operand or quotient from an exact one above 2^53.
fn exact_div_ceil(location: &str, left: &str, right: &str) -> String {
    const LEFT_MARK: &str = "\u{e003}";
    const RIGHT_MARK: &str = "\u{e004}";
    let operand_guard = format!(
        "{}{}",
        exact_guard(location, "div_ceil operand", "__ah", "__al"),
        exact_guard(location, "div_ceil operand", "__bh", "__bl")
    );
    let result_guard = exact_guard(location, "div_ceil result", "__qh", "__ql");
    format!(
        concat!(
            "(function()",
            "local __B = 4294967296;",
            "{limb_exact}",
            "local function __pack(__v) local __s = (__v < 0) and -1 or 1; if __v < 0 then __v = -__v end; local __m = math.floor(__v); local __h = math.floor(__m / __B); local __l = __m - __h * __B; return __s, __h, __l end;",
            "local function __submag(__ah, __al, __bh, __bl) local __borrow = (__al < __bl) and 1 or 0; return __ah - __bh - __borrow, __al - __bl + __borrow * __B end;",
            "local function __divmag(__ah, __al, __bh, __bl)",
            "  local __qh, __ql, __rh, __rl = 0, 0, 0, 0;",
            "  for __i = 63, 0, -1 do",
            "    local __carry = (__rl >= 2147483648) and 1 or 0;",
            "    __rh = (__rh * 2 + __carry) % __B;",
            "    __rl = (__rl * 2) % __B;",
            "    if __i >= 32 then __rl = __rl + (math.floor(__ah / 2 ^ (__i - 32)) % 2) end;",
            "    if __i < 32 then __rl = __rl + (math.floor(__al / 2 ^ __i) % 2) end;",
            "    if __rh > __bh or (__rh == __bh and __rl >= __bl) then",
            "      __rh, __rl = __submag(__rh, __rl, __bh, __bl);",
            "      if __i >= 32 then __qh = __qh + 2 ^ (__i - 32) end;",
            "      if __i < 32 then __ql = __ql + 2 ^ __i end;",
            "    end;",
            "  end;",
            "  return __qh, __ql, __rh, __rl;",
            "end;",
            "local __as, __ah, __al = __pack({LEFT_MARK});",
            "local __bs, __bh, __bl = __pack({RIGHT_MARK});",
            "{operand_guard}",
            "if __bh == 0 and __bl == 0 then error(\"divide by zero\") end;",
            "local __qh, __ql, __rh, __rl = __divmag(__ah, __al, __bh, __bl);",
            "if (__rh ~= 0 or __rl ~= 0) and __as == __bs then",
            "  local __l = __ql + 1;",
            "  local __c = math.floor(__l / __B);",
            "  __ql = __l - __c * __B;",
            "  __qh = __qh + __c;",
            "end;",
            "{result_guard}",
            "if __qh == 0 and __ql == 0 then return 0 end;",
            "return (__as * __bs) * (__qh * __B + __ql);",
            "end)()",
        ),
        LEFT_MARK = LEFT_MARK,
        RIGHT_MARK = RIGHT_MARK,
        limb_exact = LIMB_EXACT,
        operand_guard = operand_guard,
        result_guard = result_guard,
    )
    .replace(';', "\n")
    .replace(LEFT_MARK, left)
    .replace(RIGHT_MARK, right)
}

fn int_bounds(bits: usize, signed: bool) -> (i128, i128) {
    if signed {
        let max = (1i128 << (bits - 1)) - 1;
        (-max - 1, max)
    } else {
        (0, (1i128 << bits) - 1)
    }
}
fn int_shape(expr: &Expr) -> (usize, bool) {
    if let Some((_, bits, signed)) = int_literal(expr) {
        return (bits, signed);
    }
    match strip_reference(expr) {
        Expr::Path(path) => path.path.segments.iter().rev().find_map(|part| {
            let name = part.ident.to_string();
            match name.as_str() {
                "i8" | "u8" => Some((8, name.starts_with('i'))),
                "i16" | "u16" => Some((16, name.starts_with('i'))),
                "i32" | "u32" => Some((32, name.starts_with('i'))),
                "i64" | "u64" | "isize" | "usize" => {
                    Some((64, name.starts_with('i')))
                }
                _ => None,
            }
        }),
        _ => None,
    }
    .unwrap_or((64, true))
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
                let span = call.method.span().start();
                let location = format!(
                    "{}.rs:{}:{}",
                    self.module,
                    span.line,
                    span.column + 1
                );
                if let (Some((left, bits, signed)), Some((right, _, _))) =
                    (int_literal(&call.receiver), int_literal(argument))
                {
                    let (min, max) = int_bounds(bits, signed);
                    let raw = match method {
                        "saturating_add" | "wrapping_add" => left
                            .checked_add(right)
                            .unwrap_or(if signed { max } else { min }),
                        "saturating_sub" | "wrapping_sub" => left
                            .checked_sub(right)
                            .unwrap_or(if signed { min } else { max }),
                        _ => left.checked_mul(right).unwrap_or(
                            if signed && (left < 0) != (right < 0) {
                                min
                            } else {
                                max
                            },
                        ),
                    };
                    let result = if method.starts_with("wrapping_") {
                        let span = max - min + 1;
                        ((raw - min).rem_euclid(span)) + min
                    } else {
                        raw.clamp(min, max)
                    };
                    if !exact_i128(result) {
                        return Err(Error::at(
                            call.method.span(),
                            format!(
                                "`{method}` result {result} is not exactly representable in Luau (1 construct)"
                            ),
                        ));
                    }
                    Ok((format!("({result})"), ty))
                } else {
                    if has_inexact_literal(&call.receiver)
                        || has_inexact_literal(argument)
                    {
                        return Err(Error::at(
                            call.method.span(),
                            format!(
                                "`{method}` operand is not exactly representable in Luau (1 construct)"
                            ),
                        ));
                    }
                    let (bits, signed) = int_shape(&call.receiver);
                    let other = self.expr(argument)?.0;
                    let literal =
                        int_literal(argument).map(|(value, _, _)| value);
                    Ok((
                        exact_binary(
                            method, receiver, &other, literal, bits, signed,
                            &location,
                        ),
                        ty,
                    ))
                }
            }
            "div_ceil" if matches!(ty.base, Base::Int | Base::Value) => {
                let other_expr = arg(args, 0, call)?;
                if let (Some((left, bits, signed)), Some((right, _, _))) =
                    (int_literal(&call.receiver), int_literal(other_expr))
                {
                    let (minimum, _) = int_bounds(bits, signed);
                    if signed && left == minimum && right == -1 {
                        return Err(Error::at(
                            call.method.span(),
                            "integer division overflows",
                        ));
                    }
                    if !exact_i128(left) || !exact_i128(right) {
                        return Err(Error::at(
                            call.method.span(),
                            "integer division needs exact operands",
                        ));
                    }
                }
                if has_inexact_literal(&call.receiver)
                    || has_inexact_literal(other_expr)
                {
                    return Err(Error::at(
                        call.method.span(),
                        "integer division needs exact operands",
                    ));
                }
                let other = self.expr(other_expr)?.0;
                let span = call.method.span().start();
                let location = format!(
                    "{}.rs:{}:{}",
                    self.module,
                    span.line,
                    span.column + 1
                );
                Ok((exact_div_ceil(&location, receiver, &other), ty))
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
                let other_expr = arg(args, 0, call)?;
                let other = self.expr(other_expr)?.0;
                let receiver_abs = matches!(
                    strip_reference(&call.receiver),
                    Expr::MethodCall(inner) if inner.method == "abs"
                );
                let receiver_negative = matches!(
                    strip_reference(&call.receiver),
                    Expr::Unary(unary) if matches!(unary.op, UnOp::Neg(_))
                );
                let other_negative = matches!(
                    strip_reference(other_expr),
                    Expr::Unary(unary) if matches!(unary.op, UnOp::Neg(_))
                );
                let a_nan = if receiver_abs {
                    "\"Greater\""
                } else if receiver_negative {
                    "\"Less\""
                } else {
                    "\"Equal\""
                };
                let both_nan = if receiver_abs && other_negative {
                    "\"Greater\""
                } else if receiver_negative && !other_negative {
                    "\"Less\""
                } else {
                    "\"Equal\""
                };
                let b_nan = if receiver_abs {
                    "\"Less\""
                } else if receiver_negative {
                    "\"Greater\""
                } else {
                    "\"Equal\""
                };
                Ok((
                    format!(
                        "(function() local __a, __b = {receiver}, {other}; if __a ~= __a then if __b ~= __b then return {both_nan} end return {a_nan} end; if __b ~= __b then return {b_nan} end; if __a < __b then return \"Less\" end; if __a > __b then return \"Greater\" end; if __a == 0 and __b == 0 then if (1 / __a) < (1 / __b) then return \"Less\" end; if (1 / __a) > (1 / __b) then return \"Greater\" end end; return \"Equal\" end)()"
                    ),
                    Ty::VALUE,
                ))
            }
            _ => return Ok(None),
        };
        Ok(Some(value?))
    }
}
