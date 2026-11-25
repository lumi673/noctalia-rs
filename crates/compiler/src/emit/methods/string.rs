use super::*;

impl<'a> Emitter<'a> {
    pub(super) fn string_method(
        &mut self,
        call: &ExprMethodCall,
        method: &str,
        args: &[&Expr],
        receiver: &str,
        ty: Ty,
    ) -> Result<OptionalValue, Error> {
        let value: Result<(String, Ty), Error> = match method {
            "push_str" => {
                let value = self.expr(arg(args, 0, call)?)?.0;
                self.line(&format!("{receiver} = {receiver} .. ({value})"));
                Ok((String::new(), Ty::VOID))
            }
            "find" if !matches!(args.first(), Some(Expr::Closure(_))) => {
                let needle = self.expr(arg(args, 0, call)?)?.0;
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
            "is_ascii_hexdigit" => Ok((
                format!("(string.find({receiver}, \"^[0-9A-Fa-f]$\") ~= nil)"),
                Ty::BOOL,
            )),
            "rfind" => {
                let needle = self.expr(arg(args, 0, call)?)?.0;
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
                let separator = self.expr(arg(args, 0, call)?)?.0;
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
                let separator = self.expr(arg(args, 0, call)?)?.0;
                if ty.base != Base::Str {
                    return Err(Error::at(
                        call.method.span(),
                        "rsplit() needs a string receiver",
                    ));
                }
                Ok((Self::reverse_split(receiver, &separator), Ty::LIST))
            }
            "match_indices" if ty.is_string() || ty.base == Base::Value => {
                let needle = self.expr(arg(args, 0, call)?)?.0;
                Ok((
                    format!(
                        "(function() local __s, __needle = {receiver}, {needle}; local __out = {{}}; local __at = string.find(__s, __needle, 1, true); while __at do __out[#__out + 1] = {{ __at - 1, string.sub(__s, __at, __at + #__needle - 1) }}; __at = string.find(__s, __needle, __at + 1, true) end; return __out end)()"
                    ),
                    Ty::LIST,
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
                let suffix = self.expr(arg(args, 0, call)?)?.0;
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
            "is_char_boundary" => Err(Error::at(
                call.method.span(),
                format!(
                    "`{method}` has no Luau equivalent and is not supported by the Luau compiler"
                ),
            )),
            "to_string_lossy" | "display" => Ok((receiver.to_owned(), Ty::STR)),
            "strip_prefix" => {
                let prefix = self.expr(arg(args, 0, call)?)?.0;
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
                let argument = arg(args, 0, call)?;
                let pattern = self.char_class(argument, call)?;
                let leading = quote(&format!("^{pattern}+"));
                let trailing = quote(&format!("{pattern}+$"));
                let trimmed =
                    format!("string.gsub({receiver}, {leading}, \"\")");
                if method == "trim_start_matches" {
                    return Ok(Some((trimmed, Ty::STR)));
                }
                let base = if method == "trim_end_matches" {
                    receiver.to_owned()
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
                let prefix = self.expr(arg(args, 0, call)?)?.0;
                Ok((
                    format!(
                        "(string.sub({receiver}, 1, #{prefix}) == {prefix})"
                    ),
                    Ty::BOOL,
                ))
            }
            "ends_with" => {
                let suffix = self.expr(arg(args, 0, call)?)?.0;
                Ok((
                    format!("(string.sub({receiver}, -#{suffix}) == {suffix})"),
                    Ty::BOOL,
                ))
            }
            "replace" => {
                let from = self.expr(arg(args, 0, call)?)?.0;
                let to = self.expr(arg(args, 1, call)?)?.0;
                self.uses_replace = true;
                Ok((format!("replace_all({receiver}, {from}, {to})"), Ty::STR))
            }
            "split" => {
                let delimiter = self.expr(arg(args, 0, call)?)?.0;
                Ok((
                    format!(
                        "(function() local __parts = string.split({receiver}, {delimiter}); __parts.__next = 1; return __parts end)()"
                    ),
                    Ty::LIST,
                ))
            }
            "split_whitespace" => {
                Ok((self.whitespace_split(receiver), Ty::LIST))
            }
            "bytes" | "chars" => {
                Ok((format!("string.split({receiver}, \"\")"), Ty::LIST))
            }
            "lines" => Ok((self.split_lines(receiver), Ty::LIST)),
            "trim" => Ok((format!("string.trim({receiver})"), Ty::STR)),
            "to_lowercase" => {
                Ok((format!("string.lower({receiver})"), Ty::STR))
            }
            "to_uppercase" => {
                Ok((format!("string.upper({receiver})"), Ty::STR))
            }
            "contains" => {
                let (needle, _) = self.expr(arg(args, 0, call)?)?;
                Ok((
                    format!(
                        "(string.find({receiver}, {needle}, 1, true) ~= nil)"
                    ),
                    Ty::BOOL,
                ))
            }
            "split_terminator" => {
                let separator = self.expr(arg(args, 0, call)?)?.0;
                if !ty.is_string() && ty.base != Base::Value {
                    return Err(Error::at(
                        call.method.span(),
                        "split_terminator() needs a string receiver",
                    ));
                }
                Ok((
                    format!(
                        "(function() local __parts = string.split({receiver}, {separator}); if #__parts > 0 and __parts[#__parts] == \"\" then table.remove(__parts) end; __parts.__next = 1; return __parts end)()"
                    ),
                    Ty::LIST,
                ))
            }
            "rsplit_once" => {
                let separator = self.expr(arg(args, 0, call)?)?.0;
                if !ty.is_string() && ty.base != Base::Value {
                    return Err(Error::at(
                        call.method.span(),
                        "rsplit_once() needs a string receiver",
                    ));
                }
                Ok((
                    format!(
                        "(function() local __s, __sep = {receiver}, {separator}; local __at = string.find(__s, __sep, 1, true); local __last; while __at do __last = __at; __at = string.find(__s, __sep, __at + 1, true) end; if not __last then return nil end; return {{ string.sub(__s, 1, __last - 1), string.sub(__s, __last + #__sep) }} end)()"
                    ),
                    Ty::opt(Base::List),
                ))
            }
            "replacen" => {
                let from = self.expr(arg(args, 0, call)?)?.0;
                let to = self.expr(arg(args, 1, call)?)?.0;
                let count = self.expr(arg(args, 2, call)?)?.0;
                if !ty.is_string() && ty.base != Base::Value {
                    return Err(Error::at(
                        call.method.span(),
                        "replacen() needs a string receiver",
                    ));
                }
                Ok((
                    format!(
                        "(function() local __text, __from, __to, __limit = {receiver}, {from}, {to}, {count}; if __from == \"\" or __limit <= 0 then return __text end; local __out, __start, __done = {{}}, 1, 0; while __done < __limit do local __at = string.find(__text, __from, __start, true); if not __at then break end; __out[#__out + 1] = string.sub(__text, __start, __at - 1); __out[#__out + 1] = __to; __start = __at + #__from; __done = __done + 1 end; __out[#__out + 1] = string.sub(__text, __start); return table.concat(__out) end)()"
                    ),
                    Ty::STR,
                ))
            }
            "eq_ignore_ascii_case" => {
                let other = self.expr(arg(args, 0, call)?)?.0;
                Ok((
                    format!(
                        "(string.lower({receiver}) == string.lower({other}))"
                    ),
                    Ty::BOOL,
                ))
            }
            "truncate" if ty.is_string() && !ty.optional => {
                let length = self.expr(arg(args, 0, call)?)?.0;
                self.line(&format!(
                    "{receiver} = string.sub({receiver}, 1, math.max(0, math.floor({length})))"
                ));
                Ok((String::new(), Ty::VOID))
            }
            "char_indices" if ty.is_string() || ty.base == Base::Value => Ok((
                format!(
                    "(function() local __out = {{}}; for __offset, __code in utf8.codes({receiver}) do __out[#__out + 1] = {{ __offset - 1, utf8.char(__code) }} end; __out.__next = 1; return __out end)()"
                ),
                Ty::LIST,
            )),
            "cmp"
                if !matches!(
                    ty.base,
                    Base::Ops | Base::Void | Base::Context
                ) =>
            {
                let other = self.expr(arg(args, 0, call)?)?.0;
                Ok((
                    format!(
                        "(function() local __a, __b = {receiver}, {other}; if __a < __b then return \"Less\" end; if __a > __b then return \"Greater\" end; return \"Equal\" end)()"
                    ),
                    Ty::VALUE,
                ))
            }
            "splitn" => {
                let separator = self.expr(arg(args, 0, call)?)?.0;
                let limit = self.expr(arg(args, 1, call)?)?.0;
                if !ty.is_string() && ty.base != Base::Value {
                    return Err(Error::at(
                        call.method.span(),
                        "splitn() needs a string receiver",
                    ));
                }
                Ok((
                    format!(
                        "(function() local __s, __sep, __n = {receiver}, {separator}, math.max(0, math.floor({limit})); local __out = {{}}; if __n == 0 then return __out end; local __start = 1; while __n > 1 do local __at = string.find(__s, __sep, __start, true); if not __at then break end; __out[#__out + 1] = string.sub(__s, __start, __at - 1); __start = __at + #__sep; __n = __n - 1 end; __out[#__out + 1] = string.sub(__s, __start); __out.__next = 1; return __out end)()"
                    ),
                    Ty::LIST,
                ))
            }
            "repeat" if ty.is_string() || ty.base == Base::Value => {
                let count = self.expr(arg(args, 0, call)?)?.0;
                Ok((format!("string.rep({receiver}, {count})"), Ty::STR))
            }
            "len_utf8" if ty.is_string() || ty.base == Base::Value => Ok((
                format!(
                    "(function() for __offset in utf8.codes({receiver}) do if __offset > 1 then return __offset - 1 end end; return #{receiver} end)()"
                ),
                Ty::INT,
            )),
            _ => return Ok(None),
        };
        Ok(Some(value?))
    }
}
