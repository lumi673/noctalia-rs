use super::*;

impl<'a> Emitter<'a> {
    pub(super) fn range_contains(
        &mut self,
        call: &ExprMethodCall,
        args: &[&Expr],
        range: &syn::ExprRange,
    ) -> Result<(String, Ty), Error> {
        let value = self.expr(arg(args, 0, call)?)?.0;
        let start = match &range.start {
            Some(expr) => self.expr(expr)?.0,
            None => "(-math.huge)".to_owned(),
        };
        let end = match &range.end {
            Some(expr) => self.expr(expr)?.0,
            None => "math.huge".to_owned(),
        };
        let upper = match range.limits {
            syn::RangeLimits::HalfOpen(_) => "<",
            syn::RangeLimits::Closed(_) => {
                return Err(Error::at(
                    call.method.span(),
                    "inclusive ranges are not supported by the Luau compiler",
                ));
            }
        };
        Ok((
            format!(
                "(function() local __v = {value}; return (__v >= ({start}) and __v {upper} ({end})) end)()"
            ),
            Ty::BOOL,
        ))
    }
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
            "into_vec" if self.strict() => Ok((receiver.to_owned(), ty)),
            "into_owned" | "to_owned" | "clone" | "as_ref" | "as_deref"
            | "cloned" | "into_inner" | "into" | "as_mut" | "as_slice"
            | "as_bytes" | "into_bytes" | "copied" | "peekable"
            | "iter_mut" => Ok((receiver.to_owned(), ty)),
            "first" => Ok((format!("({receiver}[1])"), Ty::OPT_VALUE)),
            "last" => Ok((
                format!(
                    "(function() local __v = {receiver}; if type(__v) == \"string\" then return string.byte(__v, #__v) end; return __v[#__v] end)()"
                ),
                Ty::OPT_VALUE,
            )),
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
                if ty.base == Base::Table
                    || (ty.base == Base::Value && arity == 2)
                {
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
                } else if matches!(
                    ty.base,
                    Base::List | Base::Ops | Base::Value
                ) {
                    if arity != 1 {
                        return Err(Error::at(
                            call.method.span(),
                            "retain() on a list needs a closure with one argument",
                        ));
                    }
                    let index = self.temp();
                    let rejected = self.temp();
                    let position = self.temp();
                    self.line(&format!("local {rejected} = {{}}"));
                    self.line(&format!("for {index} = 1, #{receiver} do"));
                    self.indent = self.indent.saturating_add(1);
                    let item = self.temp();
                    self.line(&format!("local {item} = {receiver}[{index}]"));
                    let (text, _) =
                        self.invoke_arg(closure, std::slice::from_ref(&item))?;
                    self.line(&format!(
                        "if not ({text}) then {rejected}[#{rejected} + 1] = {index} end"
                    ));
                    self.indent = self.indent.saturating_sub(1);
                    self.line("end");
                    self.line(&format!(
                        "for {position} = #{rejected}, 1, -1 do table.remove({receiver}, {rejected}[{position}]) end"
                    ));
                } else {
                    return Err(Error::at(
                        call.method.span(),
                        "retain() needs a list or map receiver",
                    ));
                }
                Ok((String::new(), Ty::VOID))
            }
            "sort"
                if matches!(ty.base, Base::List | Base::Ops | Base::Value)
                    && args.is_empty() =>
            {
                let location = call.method.span().start();
                let indices = self.temp();
                let left = self.temp();
                let right = self.temp();
                let sorted = self.temp();
                let position = self.temp();
                self.line(&format!("local {indices} = {{}}"));
                self.line(&format!(
                    "for {position} = 1, #{receiver} do {indices}[{position}] = {position} end"
                ));
                self.line(&format!(
                    "table.sort({indices}, function({left}, {right})"
                ));
                self.indent = self.indent.saturating_add(1);
                self.line(&format!(
                    "local __ka, __kb = {receiver}[{left}], {receiver}[{right}]; if type(__ka) ~= \"string\" and type(__ka) ~= \"number\" then error(\"{module}:{line}:{column}: unsupported sort key\") end; if __ka < __kb then return true end",
                    module = self.module,
                    line = location.line,
                    column = location.column + 1
                ));
                self.line("if __kb < __ka then return false end");
                self.line(&format!("return {left} < {right}"));
                self.indent = self.indent.saturating_sub(1);
                self.line("end)");
                self.line(&format!("local {sorted} = {{}}"));
                self.line(&format!(
                    "for {position} = 1, #{indices} do {sorted}[{position}] = {receiver}[{indices}[{position}]] end"
                ));
                self.line(&format!(
                    "for {position} = 1, #{sorted} do {receiver}[{position}] = {sorted}[{position}] end"
                ));
                Ok((String::new(), Ty::VOID))
            }
            "sort_by"
                if matches!(ty.base, Base::List | Base::Ops | Base::Value) =>
            {
                let closure = arg(args, 0, call)?;
                let indices = self.temp();
                let left = self.temp();
                let right = self.temp();
                let sorted = self.temp();
                let position = self.temp();
                self.line(&format!("local {indices} = {{}}"));
                self.line(&format!(
                    "for {position} = 1, #{receiver} do {indices}[{position}] = {position} end"
                ));
                self.line(&format!(
                    "table.sort({indices}, function({left}, {right})"
                ));
                self.indent = self.indent.saturating_add(1);
                self.line("do");
                self.indent = self.indent.saturating_add(1);
                let (order, _) = self.invoke_arg(
                    closure,
                    &[
                        format!("{receiver}[{left}]"),
                        format!("{receiver}[{right}]"),
                    ],
                )?;
                self.line(&format!(
                    "if ({order}) == \"Less\" then return true end"
                ));
                self.line(&format!(
                    "if ({order}) == \"Greater\" then return false end"
                ));
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                self.line(&format!("return {left} < {right}"));
                self.indent = self.indent.saturating_sub(1);
                self.line("end)");
                self.line(&format!("local {sorted} = {{}}"));
                self.line(&format!(
                    "for {position} = 1, #{indices} do {sorted}[{position}] = {receiver}[{indices}[{position}]] end"
                ));
                self.line(&format!(
                    "for {position} = 1, #{sorted} do {receiver}[{position}] = {sorted}[{position}] end"
                ));
                Ok((String::new(), Ty::VOID))
            }
            "sort_by_key"
                if matches!(ty.base, Base::List | Base::Ops | Base::Value) =>
            {
                let location = call.method.span().start();
                let closure = arg(args, 0, call)?;
                let indices = self.temp();
                let keys = self.temp();
                let left = self.temp();
                let right = self.temp();
                let sorted = self.temp();
                let position = self.temp();
                self.line(&format!("local {indices}, {keys} = {{}}, {{}}"));
                self.line(&format!("for {position} = 1, #{receiver} do"));
                self.indent = self.indent.saturating_add(1);
                self.line("do");
                self.indent = self.indent.saturating_add(1);
                let (key, _) = self.invoke_arg(
                    closure,
                    std::slice::from_ref(&format!("{receiver}[{position}]")),
                )?;
                self.line(&format!(
                    "if type({key}) ~= \"string\" and type({key}) ~= \"number\" then error(\"{}:{}:{}: unsupported sort key\") end; {keys}[{position}] = {key}",
                    self.module,
                    location.line,
                    location.column + 1,
                ));
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                self.line(&format!(
                    "for {position} = 1, #{receiver} do {indices}[{position}] = {position} end"
                ));
                self.line(&format!(
                    "table.sort({indices}, function({left}, {right})"
                ));
                self.indent = self.indent.saturating_add(1);
                self.line(&format!(
                    "if ({keys}[{left}]) < ({keys}[{right}]) then return true end"
                ));
                self.line(&format!(
                    "if ({keys}[{right}]) < ({keys}[{left}]) then return false end"
                ));
                self.line(&format!("return {left} < {right}"));
                self.indent = self.indent.saturating_sub(1);
                self.line("end)");
                self.line(&format!("local {sorted} = {{}}"));
                self.line(&format!(
                    "for {position} = 1, #{indices} do {sorted}[{position}] = {receiver}[{indices}[{position}]] end"
                ));
                self.line(&format!(
                    "for {position} = 1, #{sorted} do {receiver}[{position}] = {sorted}[{position}] end"
                ));
                Ok((String::new(), Ty::VOID))
            }
            "sort_unstable_by" | "sort_unstable_by_key" => {
                return Err(Error::at(
                    call.method.span(),
                    format!(
                        "`{method}` has no Luau equivalent: it reorders equal elements"
                    ),
                ));
            }
            "then" if matches!(args.first(), Some(Expr::Closure(_))) => {
                let closure = arg(args, 0, call)?;
                let flag = self.temp();
                let result = self.temp();
                self.line(&format!("local {flag} = {receiver}"));
                self.line(&format!("local {result}"));
                self.line(&format!("if {flag} then"));
                self.indent = self.indent.saturating_add(1);
                let (text, value_ty) = self.invoke_arg(closure, &[])?;
                self.line(&format!("{result} = {text}"));
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                Ok((result, Ty::opt(value_ty.base)))
            }
            "then_with" => {
                let closure = arg(args, 0, call)?;
                let order = self.temp();
                self.line(&format!("local {order} = {receiver}"));
                self.line(&format!("if {order} == \"Equal\" then"));
                self.indent = self.indent.saturating_add(1);
                let (text, _) = self.invoke_arg(closure, &[])?;
                self.line(&format!("{order} = {text}"));
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                Ok((order, Ty::VALUE))
            }
            "reverse" if ty.base == Base::Value => {
                let order = self.temp();
                let mapping = "(function(__o) if __o == \"Less\" then return \"Greater\" elseif __o == \"Greater\" then return \"Less\" end return __o end)";
                self.line(&format!("local {order} = {mapping}({receiver})"));
                Ok((order, Ty::VALUE))
            }
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
                Ok((super::path::path_join(receiver, &part), Ty::PATH))
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
                    "local {cursor} = ((type({meta}) == \"table\" and {meta}.{CURSOR_MARKER} ~= nil and {meta}.{CURSOR_MARKER}) or (type({meta}) == \"table\" and {meta}.__noctalia_array and 0) or {receiver}.__next or 0) + 1"
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
                let parsed_ty = turbofish_type(call.turbofish.as_ref());
                let target = call
                    .turbofish
                    .as_ref()
                    .and_then(|arguments| match arguments.args.first() {
                        Some(syn::GenericArgument::Type(ty)) => {
                            Some(type_text(ty))
                        }
                        _ => None,
                    })
                    .unwrap_or_default();
                let (positive_limit, negative_limit, signed) = match target
                    .as_str()
                {
                    "i8" => ("127", "128", true),
                    "i16" => ("32767", "32768", true),
                    "i32" => ("2147483647", "2147483648", true),
                    "u8" => ("255", "0", false),
                    "u16" => ("65535", "0", false),
                    "u32" => ("4294967295", "0", false),
                    "u64" | "usize" => ("18446744073709551615", "0", false),
                    _ => ("9223372036854775807", "9223372036854775808", true),
                };
                let body = match parsed_ty.base {
                    Base::Int | Base::Value => {
                        if let Some(literal) = literal_string(&call.receiver) {
                            let digits = literal
                                .trim_start_matches(['+', '-'])
                                .trim_start_matches('0');
                            let negative = literal.starts_with('-');
                            let limit = if negative {
                                negative_limit
                            } else {
                                positive_limit
                            };
                            if (negative && !signed)
                                || digits.len() > 16
                                || (digits.len() == 16
                                    && digits > "9007199254740992")
                                || digits.len() > limit.len()
                            {
                                return Err(Error::at(
                                    call.method.span(),
                                    "parse() value cannot be represented exactly in Luau",
                                ));
                            }
                        }
                        let exact = format!(
                            "(function() local __s = {receiver}; local __start, __finish, __sign, __digits = string.find(__s, \"^([+-]?)(%d+)$\"); if __start == nil then return nil, \"invalid digit found in string\" end; __digits = string.gsub(__digits, \"^0+\", \"\"); if __digits == \"\" then __digits = \"0\" end; if __sign == \"-\" and not {signed} then return nil, \"number too large or small\" end; local __limit = (__sign == \"-\") and \"{negative_limit}\" or \"{positive_limit}\"; if #__digits > #__limit or (#__digits == #__limit and __digits > __limit) then return nil, \"number too large or small\" end; local __exact = \"9007199254740992\"; if #__digits > #__exact or (#__digits == #__exact and __digits > __exact) then return nil, \"number cannot be represented exactly\" end; local __n = tonumber(__s); if __n == nil then return nil, \"invalid digit found in string\" end; return math.floor(__n) end)()",
                        );
                        if parsed_ty.base == Base::Int {
                            exact
                        } else {
                            format!(
                                "(function() local __s = {receiver}; if string.find(__s, \"^[+-]?%d+$\") ~= nil then return {exact} end; if string.find(__s, \"^[+-]?%d+%.%d*$\") == nil and string.find(__s, \"^[+-]?%d+[eE][+-]?%d+$\") == nil and string.find(__s, \"^[+-]?%.%d+$\") == nil and string.find(__s, \"^[+-]?%.%d+[eE][+-]?%d+$\") == nil then return nil, \"invalid number literal\" end; local __n = tonumber(__s); if __n == nil then return nil, \"invalid number literal\" end; return __n end)()"
                            )
                        }
                    }
                    Base::Float => format!(
                        "(function() local __s = {receiver}; if string.find(__s, \"^[+-]?%d+%.%d*$\") == nil and string.find(__s, \"^[+-]?%d+[eE][+-]?%d+$\") == nil and string.find(__s, \"^[+-]?%.%d+$\") == nil and string.find(__s, \"^[+-]?%.%d+[eE][+-]?%d+$\") == nil then return nil, \"invalid float literal\" end; local __n = tonumber(__s); if __n == nil then return nil, \"invalid float literal\" end; return __n end)()"
                    ),
                    _ => {
                        return Err(Error::at(
                            call.method.span(),
                            "parse::<T>() has no faithful Luau lowering for this type",
                        ));
                    }
                };
                Ok((body, Ty::result(parsed_ty.base)))
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
                let fallback = arg(args, 0, call)?;
                let (default, default_ty) =
                    match path_last_name(fallback).as_deref() {
                        Some(name) if name.ends_with("Ordering::Equal") => {
                            ("\"Equal\"".to_owned(), Ty::VALUE)
                        }
                        Some(name) if name.ends_with("Ordering::Less") => {
                            ("\"Less\"".to_owned(), Ty::VALUE)
                        }
                        Some(name) if name.ends_with("Ordering::Greater") => {
                            ("\"Greater\"".to_owned(), Ty::VALUE)
                        }
                        _ => self.expr(fallback)?,
                    };
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
                let mut out = Ty::new(wider(ty, default_ty).base);
                out.null = default == "nil" && ty.null;
                Ok((result, out))
            }
            "unwrap_or_default" => {
                let nil_default = ty.base == Base::Value;
                let default = if nil_default {
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
                    let mut out = Ty::new(ty.base);
                    out.null = nil_default && ty.null;
                    return Ok(Some((result, out)));
                }
                let value = self.temp();
                let result = self.temp();
                self.line(&format!("local {value} = {receiver}"));
                self.line(&format!("local {result} = {value}"));
                self.line(&format!(
                    "if {value} == nil then {result} = {default} end"
                ));
                let mut out = Ty::new(ty.base);
                out.null = nil_default && ty.null;
                Ok((result, out))
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
