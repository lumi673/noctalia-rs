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
            "match_indices" if ty.base == Base::Str => {
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
            "is_char_boundary" | "char_indices" => Err(Error::at(
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
            _ => return Ok(None),
        };
        Ok(Some(value?))
    }
}
