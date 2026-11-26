use super::*;

impl<'a> Emitter<'a> {
    pub(super) fn macro_expr(
        &mut self,
        mac: &ExprMacro,
    ) -> Result<(String, Ty), Error> {
        match macro_name(mac).as_str() {
            "format" => self.format(mac),
            "write" => self.write_macro(mac, false),
            "writeln" => self.write_macro(mac, true),
            "matches" => self.matches_macro(mac),
            "json" => self.json(mac),
            "vec" => {
                let items = parse_vec(mac)?;
                if items.iter().any(is_op_expr) {
                    return Err(Error::at(
                        mac.span(),
                        "vec! of ops is only supported as Ok(vec![...]) or a let binding",
                    ));
                }
                self.list(&items)
            }
            other => Err(Error::at(
                mac.span(),
                format!(
                    "macro `{other}!` is not supported by the Luau compiler"
                ),
            )),
        }
    }

    pub(super) fn format(
        &mut self,
        mac: &ExprMacro,
    ) -> Result<(String, Ty), Error> {
        let parsed: FormatArgs = syn::parse2(mac.mac.tokens.clone())?;
        let text = self.format_text(mac, &parsed)?;
        Ok((text, Ty::STR))
    }

    /// The `format!` body, shared with `write!` and `writeln!`.
    pub(crate) fn format_text(
        &mut self,
        mac: &ExprMacro,
        parsed: &FormatArgs,
    ) -> Result<String, Error> {
        let mut pieces = Vec::new();
        let mut text = String::new();
        let mut chars = parsed.literal.chars().peekable();
        let mut positional =
            parsed.arguments.iter().filter(|(name, _)| name.is_none());
        while let Some(character) = chars.next() {
            match character {
                '{' => {
                    if chars.peek() == Some(&'{') {
                        chars.next();
                        text.push('{');
                        continue;
                    }
                    let mut placeholder = String::new();
                    for inner in chars.by_ref() {
                        if inner == '}' {
                            break;
                        }
                        placeholder.push(inner);
                    }
                    if !text.is_empty() {
                        pieces.push(quote(&text));
                        text.clear();
                    }
                    let (placeholder, spec) = match placeholder.split_once(':')
                    {
                        Some((name, spec)) => {
                            (name.to_owned(), Some(spec.to_owned()))
                        }
                        None => (placeholder, None),
                    };
                    let (value, ty) = if placeholder.is_empty() {
                        let (_, expr) = positional.next().ok_or_else(|| {
                            Error::at(
                                mac.span(),
                                "format! has more placeholders than arguments",
                            )
                        })?;
                        self.expr(expr)?
                    } else if let Some((_, expr)) = parsed
                        .arguments
                        .iter()
                        .find(|(key, _)| key.as_deref() == Some(&placeholder))
                    {
                        self.expr(expr)?
                    } else if let Some(ty) = self.lookup(&placeholder) {
                        (self.local_name(&placeholder), ty)
                    } else if let Some((text, ty)) = self.constant(&placeholder)
                    {
                        (text, ty)
                    } else if let Some(value) = self
                        .program
                        .imported_constant(&self.module, &placeholder)
                        && let Some((text, ty)) = constant(value)
                    {
                        (text, ty)
                    } else {
                        return Err(Error::at(
                            mac.span(),
                            format!(
                                "format! placeholder `{placeholder}` has no matching argument"
                            ),
                        ));
                    };
                    match spec {
                        Some(spec) => pieces
                            .push(format_specifier(value, ty, &spec, mac)?),
                        None => pieces.push(format_piece(value, ty)),
                    }
                }
                '}' => {
                    if chars.peek() == Some(&'}') {
                        chars.next();
                    }
                    text.push('}');
                }
                other => text.push(other),
            }
        }
        if !text.is_empty() {
            pieces.push(quote(&text));
        }
        if pieces.is_empty() {
            pieces.push("\"\"".to_owned());
        }
        Ok(pieces.join(" .. "))
    }

    pub(super) fn json(
        &mut self,
        mac: &ExprMacro,
    ) -> Result<(String, Ty), Error> {
        let node: JsonNode = syn::parse2(mac.mac.tokens.clone())?;
        self.json_node(&node)
    }

    fn matches_macro(
        &mut self,
        mac: &ExprMacro,
    ) -> Result<(String, Ty), Error> {
        let parsed: MatchesArgs = syn::parse2(mac.mac.tokens.clone())?;
        self.record_pattern_events(&parsed.expr, &parsed.pat);
        let (expr, ty) = self.expr(&parsed.expr)?;
        let Some(guard) = parsed.guard else {
            let (condition, _) =
                self.pattern_match(&expr, &parsed.pat, None)?;
            return Ok((format!("({condition})"), Ty::BOOL));
        };
        let value = self.temp();
        let result = self.temp();
        let saved_out = std::mem::take(&mut self.out);
        let saved_indent = self.indent;
        self.indent = 0;
        self.line(&format!("local {value} = {expr}"));
        self.line(&format!("local {result} = false"));
        let (condition, bindings) =
            self.pattern_match(&value, &parsed.pat, None)?;
        self.line(&format!("if {condition} then"));
        self.indent = self.indent.saturating_add(1);
        self.push_scope();
        for (name, text) in bindings {
            let binding = self.binding_ty(&parsed.pat, &name, ty);
            let lua = self.bind_name(&name);
            self.line(&format!("local {lua} = {text}"));
            self.push(&name, binding);
        }
        let guard = self.expr(&guard);
        self.pop_scope();
        let (guard, _) = guard?;
        self.line(&format!("{result} = ({guard})"));
        self.indent = self.indent.saturating_sub(1);
        self.line("end");
        self.line(&format!("return {result}"));
        let body = std::mem::replace(&mut self.out, saved_out);
        self.indent = saved_indent;
        Ok((format!("(function()\n{body}end)()"), Ty::BOOL))
    }

    fn json_node(&mut self, node: &JsonNode) -> Result<(String, Ty), Error> {
        match node {
            JsonNode::Object(entries) => {
                let mut parts = Vec::new();
                for (key, value) in entries {
                    let (text, _) = self.json_node(value)?;
                    if text == "nil" {
                        return Err(Error::at(
                            value.span(),
                            "json null inside an object is not supported; omit the key instead",
                        ));
                    }
                    parts.push(format!("{} = {text}", lua_key(key)));
                }
                Ok((format!("{{ {} }}", parts.join(", ")), Ty::VALUE))
            }
            JsonNode::Array(values) => {
                let mut parts = Vec::new();
                for value in values {
                    let (text, _) = self.json_node(value)?;
                    if text == "nil" {
                        return Err(Error::at(
                            value.span(),
                            "json null inside an array is not supported",
                        ));
                    }
                    parts.push(text);
                }
                if parts.is_empty() {
                    return Ok((empty_array(), Ty::VALUE));
                }
                Ok((format!("{{ {} }}", parts.join(", ")), Ty::VALUE))
            }
            JsonNode::Expr(expr) => {
                if let Expr::Path(path) = &**expr
                    && path.path.is_ident("null")
                {
                    return Ok(("nil".to_owned(), Ty::NULL));
                }
                let (text, ty) = self.expr(expr)?;
                if ty.is_numeric() {
                    return Ok((text, ty));
                }
                Ok((text, Ty::VALUE))
            }
        }
    }
}

/// One `{value:spec}` placeholder of a `format!` call.
fn format_specifier(
    value: String,
    ty: Ty,
    spec: &str,
    mac: &ExprMacro,
) -> Result<String, Error> {
    let unsupported = || {
        Error::at(
            mac.span(),
            format!(
                "format specifier `{spec}` is not supported by the Luau compiler"
            ),
        )
    };
    if let Some(debug) = spec.strip_suffix('?') {
        if !debug.is_empty() {
            return Err(unsupported());
        }
        match ty.base {
            Base::Int | Base::Float | Base::Bool => {
                return Ok(format!("tostring({value})"));
            }
            Base::Value => {
                return Err(Error::at(
                    mac.span(),
                    "`{:?}` on a JSON value has no Luau equivalent: Rust prints `String(\"x\")` and `Number(1)`",
                ));
            }
            _ => return Err(unsupported()),
        }
    }
    let mut left = ty.base == Base::Str;
    let mut plus = false;
    let mut rest = spec;
    while let Some(first) = rest.chars().next() {
        match first {
            '<' => left = true,
            '>' => left = false,
            '+' => plus = true,
            _ => break,
        }
        rest = &rest[first.len_utf8()..];
    }
    if !rest
        .chars()
        .all(|character| character.is_ascii_digit() || character == '.')
    {
        return Err(unsupported());
    }
    if plus && !ty.is_numeric() {
        return Err(unsupported());
    }
    let mut format = String::from("%");
    if plus {
        format.push('+');
    }
    if left {
        format.push('-');
    }
    format.push_str(rest);
    let conversion = match ty.base {
        Base::Str | Base::Path => 's',
        Base::Int => 'd',
        Base::Float => {
            if rest.contains('.') {
                'f'
            } else {
                'd'
            }
        }
        Base::Bool | Base::Value | Base::List | Base::Table | Base::Node => 's',
        Base::Info | Base::Ops | Base::Context | Base::Void => {
            return Err(unsupported());
        }
    };
    format.push(conversion);
    let value = match ty.base {
        Base::Str | Base::Path | Base::Int | Base::Float => value,
        _ => format!("tostring({value})"),
    };
    Ok(format!("string.format({}, {value})", quote(&format)))
}

fn format_piece(value: String, ty: Ty) -> String {
    if ty.is_string() || ty.is_numeric() {
        value
    } else if ty.base == Base::Value {
        format!("(json.encode({value}))")
    } else {
        format!("tostring({value})")
    }
}

pub(super) fn macro_name(mac: &ExprMacro) -> String {
    mac.mac
        .path
        .segments
        .last()
        .map_or_else(String::new, |segment| segment.ident.to_string())
}

pub(super) fn parse_vec(mac: &ExprMacro) -> Result<Vec<Expr>, Error> {
    let list = syn::parse::Parser::parse2(
        Punctuated::<Expr, Token![,]>::parse_terminated,
        mac.mac.tokens.clone(),
    )?;
    Ok(list.into_iter().collect())
}

pub(crate) struct MatchesArgs {
    pub(crate) expr: Expr,
    pub(crate) pat: Pat,
    pub(crate) guard: Option<Expr>,
}

impl Parse for MatchesArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let expr: Expr = input.parse()?;
        let _: Token![,] = input.parse()?;
        let pat: Pat = Pat::parse_multi(input)?;
        let guard = if input.peek(Token![if]) {
            let _: Token![if] = input.parse()?;
            Some(input.parse()?)
        } else {
            None
        };
        Ok(Self { expr, pat, guard })
    }
}

pub(crate) struct FormatArgs {
    pub(crate) literal: String,
    pub(crate) arguments: Vec<(Option<String>, Expr)>,
}

impl Parse for FormatArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let literal: syn::LitStr = input.parse()?;
        let mut arguments = Vec::new();
        while !input.is_empty() {
            let _: Token![,] = input.parse()?;
            if input.is_empty() {
                break;
            }
            let mut name = None;
            if input.peek(syn::Ident) && input.peek2(Token![=]) {
                let ident: syn::Ident = input.parse()?;
                let _: Token![=] = input.parse()?;
                name = Some(ident.to_string());
            }
            let expr: Expr = input.parse()?;
            arguments.push((name, expr));
        }
        Ok(Self {
            literal: literal.value(),
            arguments,
        })
    }
}

pub(crate) enum JsonNode {
    Object(Vec<(String, JsonNode)>),
    Array(Vec<JsonNode>),
    Expr(Box<Expr>),
}

impl Parse for JsonNode {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        if input.peek(syn::token::Brace) {
            let content;
            syn::braced!(content in input);
            let mut entries = Vec::new();
            while !content.is_empty() {
                let key = if content.peek(syn::LitStr) {
                    content.parse::<syn::LitStr>()?.value()
                } else {
                    content.parse::<syn::Ident>()?.to_string()
                };
                let _: Token![:] = content.parse()?;
                let value: JsonNode = content.parse()?;
                entries.push((key, value));
                if content.peek(Token![,]) {
                    let _: Token![,] = content.parse()?;
                }
            }
            return Ok(Self::Object(entries));
        }
        if input.peek(syn::token::Bracket) {
            let content;
            syn::bracketed!(content in input);
            let mut values = Vec::new();
            while !content.is_empty() {
                values.push(content.parse()?);
                if content.peek(Token![,]) {
                    let _: Token![,] = content.parse()?;
                }
            }
            return Ok(Self::Array(values));
        }
        Ok(Self::Expr(Box::new(input.parse()?)))
    }
}

impl JsonNode {
    pub(super) fn span(&self) -> proc_macro2::Span {
        match self {
            Self::Object(entries) => entries
                .first()
                .map_or_else(proc_macro2::Span::call_site, |(_, value)| {
                    value.span()
                }),
            Self::Array(values) => values
                .first()
                .map_or_else(proc_macro2::Span::call_site, |value| {
                    value.span()
                }),
            Self::Expr(expr) => expr.span(),
        }
    }
}

/// The global name in a `json!` object's `onAction` field, when literal.
pub(super) fn json_action_name(expr: &Expr) -> Option<String> {
    let Expr::Macro(mac) = strip_reference(expr) else {
        return None;
    };
    if macro_name(mac) != "json" {
        return None;
    }
    let node: JsonNode = syn::parse2(mac.mac.tokens.clone()).ok()?;
    let JsonNode::Object(entries) = node else {
        return None;
    };
    entries.into_iter().find_map(|(key, value)| {
        if key != "onAction" {
            return None;
        }
        let JsonNode::Expr(expr) = value else {
            return None;
        };
        literal_string(&expr)
    })
}
