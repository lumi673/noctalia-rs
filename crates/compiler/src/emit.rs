use std::collections::{BTreeSet, HashMap};

use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{
    BinOp, Expr, ExprBinary, ExprCall, ExprLit, ExprMacro, ExprMethodCall, Lit,
    Member, Pat, Stmt, Token, Type, UnOp,
};

use crate::{Error, is_identifier, quote};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Base {
    Str,
    Int,
    Float,
    Bool,
    Value,
    List,
    Node,
    Table,
    Ops,
    Void,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Ty {
    base: Base,
    optional: bool,
}

impl Ty {
    const fn new(base: Base) -> Self {
        Self {
            base,
            optional: false,
        }
    }

    const fn opt(base: Base) -> Self {
        Self {
            base,
            optional: true,
        }
    }

    const STR: Self = Self::new(Base::Str);
    const OPT_STR: Self = Self::opt(Base::Str);
    const INT: Self = Self::new(Base::Int);
    const OPT_INT: Self = Self::opt(Base::Int);
    const FLOAT: Self = Self::new(Base::Float);
    const OPT_FLOAT: Self = Self::opt(Base::Float);
    const BOOL: Self = Self::new(Base::Bool);
    const OPT_BOOL: Self = Self::opt(Base::Bool);
    const VALUE: Self = Self::new(Base::Value);
    const OPT_VALUE: Self = Self::opt(Base::Value);
    const LIST: Self = Self::new(Base::List);
    const OPT_LIST: Self = Self::opt(Base::List);
    const NODE: Self = Self::new(Base::Node);
    const TABLE: Self = Self::new(Base::Table);
    const OPS: Self = Self::new(Base::Ops);
    const VOID: Self = Self::new(Base::Void);

    const fn is_numeric(self) -> bool {
        matches!(self.base, Base::Int | Base::Float)
    }

    fn is_string(self) -> bool {
        self.base == Base::Str
    }

    const fn default_literal(self) -> Option<&'static str> {
        match self.base {
            Base::Str => Some("\"\""),
            Base::Int | Base::Float => Some("0"),
            Base::Bool => Some("false"),
            Base::List | Base::Table => Some("{}"),
            Base::Value | Base::Node | Base::Ops | Base::Void => None,
        }
    }
}

pub(crate) type PayloadParam = (&'static str, &'static str, &'static str);

/// Payload keys that map directly to a callback parameter, so the generated
/// body can take the argument instead of building and reading a payload table.
pub(crate) fn payload_params(event: &str) -> &'static [PayloadParam] {
    match event {
        "activate" => &[("id", "arg_id", "id")],
        "search" | "search_change" | "query" => &[("text", "arg_text", "text")],
        "ipc" => &[
            ("event", "arg_event", "event"),
            ("payload", "arg_payload", "payload"),
        ],
        "scroll" => &[("axis", "arg_axis", "axis")],
        "key" => &[("chord", "arg_chord", "chord")],
        "open" => &[("context", "arg_context", "context")],
        "audio_spectrum" => &[
            ("values_csv", "arg_values_csv", "valuesCsv"),
            ("state_csv", "arg_state_csv", "stateCsv"),
        ],
        _ => &[],
    }
}

#[derive(Default)]
pub(crate) struct Body {
    pub source: String,
    pub events: BTreeSet<String>,
    pub uses_state: bool,
    pub uses_entry: bool,
    pub uses_payload: bool,
    pub payload_escapes: bool,
    pub uses_irem: bool,
    pub uses_unwrap: bool,
    pub uses_host_call: bool,
    pub uses_bool_or: bool,
    pub uses_source: bool,
    pub params: Vec<&'static str>,
    pub is_empty: bool,
}

pub(crate) struct Emitter {
    ctx: String,
    out: String,
    indent: usize,
    scopes: Vec<HashMap<String, Ty>>,
    events: BTreeSet<String>,
    bound_event: Option<String>,
    payload_params: &'static [PayloadParam],
    params: Vec<&'static str>,
    strings: Vec<String>,
    constants: HashMap<String, (String, Ty)>,
    uses_state: bool,
    uses_payload: bool,
    payload_escapes: bool,
    uses_entry: bool,
    uses_irem: bool,
    uses_unwrap: bool,
    uses_host_call: bool,
    uses_bool_or: bool,
    uses_source: bool,
    emitted: bool,
    temp: usize,
}

impl Emitter {
    pub(crate) fn new(
        ctx: &syn::Ident,
        bound_event: Option<String>,
        strings: &[&str],
        payload_params: &'static [PayloadParam],
    ) -> Self {
        Self {
            ctx: ctx.to_string(),
            out: String::new(),
            indent: 1,
            scopes: vec![HashMap::new()],
            events: BTreeSet::new(),
            bound_event,
            payload_params,
            params: Vec::new(),
            strings: strings.iter().map(|key| (*key).to_owned()).collect(),
            constants: HashMap::new(),
            uses_state: false,
            uses_payload: false,
            payload_escapes: false,
            uses_entry: false,
            uses_irem: false,
            uses_unwrap: false,
            uses_host_call: false,
            uses_bool_or: false,
            uses_source: false,
            emitted: false,
            temp: 0,
        }
    }

    pub(crate) fn constants(&mut self, items: &[syn::Item]) {
        for item in items {
            match item {
                syn::Item::Const(item_const) => {
                    let Expr::Lit(literal) = &*item_const.expr else {
                        continue;
                    };
                    let value = match &literal.lit {
                        Lit::Str(value) => (quote(&value.value()), Ty::STR),
                        Lit::Int(value) => {
                            (value.base10_digits().to_owned(), Ty::INT)
                        }
                        Lit::Bool(value) => (
                            if value.value { "true" } else { "false" }
                                .to_owned(),
                            Ty::BOOL,
                        ),
                        _ => continue,
                    };
                    self.constants.insert(item_const.ident.to_string(), value);
                }
                syn::Item::Mod(module) => {
                    if let Some((_, items)) = &module.content {
                        self.constants(items);
                    }
                }
                _ => {}
            }
        }
    }

    pub(crate) fn block(&mut self, block: &syn::Block) -> Result<(), Error> {
        self.scopes.push(HashMap::new());
        let result = self.block_inner(block, true);
        self.scopes.pop();
        result
    }

    fn block_inner(
        &mut self,
        block: &syn::Block,
        tail: bool,
    ) -> Result<(), Error> {
        let Some((last, rest)) = block.stmts.split_last() else {
            return Ok(());
        };
        for stmt in rest {
            self.stmt(stmt)?;
        }
        if tail {
            self.tail_stmt(last)
        } else {
            self.stmt(last)
        }
    }

    pub(crate) fn finish(self) -> Body {
        let mut source = String::new();
        if self.uses_state {
            source.push_str(
                "  local plugin_state = state.get(STATE_KEY) or {}\n",
            );
        }
        if self.uses_entry {
            source.push_str("  local entry = ENTRY\n");
        }
        source.push_str(&self.out);
        Body {
            source,
            events: self.events,
            uses_state: self.uses_state,
            uses_entry: self.uses_entry,
            uses_payload: self.uses_payload,
            payload_escapes: self.payload_escapes,
            uses_irem: self.uses_irem,
            uses_unwrap: self.uses_unwrap,
            uses_host_call: self.uses_host_call,
            uses_bool_or: self.uses_bool_or,
            uses_source: self.uses_source,
            params: self.params.clone(),
            is_empty: !self.emitted,
        }
    }

    fn line(&mut self, text: &str) {
        for _ in 0..self.indent {
            self.out.push_str("  ");
        }
        self.out.push_str(text);
        self.out.push('\n');
    }

    fn push(&mut self, name: &str, ty: Ty) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_owned(), ty);
        }
    }

    fn lookup(&self, name: &str) -> Option<Ty> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }

    fn stmt(&mut self, stmt: &Stmt) -> Result<(), Error> {
        match stmt {
            Stmt::Local(local) => self.local(local),
            Stmt::Expr(expr, _) if diverges(expr) => {
                self.line("do");
                self.indent = self.indent.saturating_add(1);
                self.expr_stmt(expr)?;
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                Ok(())
            }
            Stmt::Expr(expr, _) => self.expr_stmt(expr),
            other => Err(Error::at(
                other.span(),
                "unsupported statement in a compilable handler",
            )),
        }
    }

    fn tail_stmt(&mut self, stmt: &Stmt) -> Result<(), Error> {
        match stmt {
            Stmt::Expr(expr, _) => self.tail_expr(expr),
            other => self.stmt(other),
        }
    }

    fn tail_expr(&mut self, expr: &Expr) -> Result<(), Error> {
        match expr {
            Expr::Call(call) if is_ok_call(call) => self.ok_tail(&call.args),
            Expr::If(item) => self.if_stmt(item, true),
            Expr::Match(item) => self.match_stmt(item, true),
            Expr::Block(block) => self.block(&block.block),
            Expr::Paren(paren) => self.tail_expr(&paren.expr),
            Expr::Group(group) => self.tail_expr(&group.expr),
            Expr::Tuple(tuple) if tuple.elems.is_empty() => Ok(()),
            _ => {
                let (text, ty) = self.expr(expr)?;
                if ty.base != Base::Void {
                    self.line(&format!("return {text}"));
                }
                Ok(())
            }
        }
    }

    fn local(&mut self, local: &syn::Local) -> Result<(), Error> {
        let (pattern, hint) = match &local.pat {
            Pat::Type(typed) => (&*typed.pat, Some(&*typed.ty)),
            other => (other, None),
        };
        let name = match pattern {
            Pat::Ident(pattern) => pattern.ident.to_string(),
            Pat::Wild(_) => "_".to_owned(),
            _ => {
                return Err(Error::at(
                    local.pat.span(),
                    "only plain `let` bindings are supported",
                ));
            }
        };
        let Some(init) = &local.init else {
            let ty = hint.map_or(Ty::VALUE, type_hint);
            self.push(&name, ty);
            self.line(&format!("local {name} = nil"));
            return Ok(());
        };
        if let Expr::Call(call) = &*init.expr
            && is_vec_new(call)
        {
            self.line(&format!("local {name} = {{}}"));
            self.push(&name, Ty::OPS);
            return Ok(());
        }
        if let Expr::Macro(mac) = &*init.expr
            && macro_name(mac) == "vec"
        {
            let items = parse_vec(mac)?;
            if items.iter().any(is_op_expr) {
                for item in &items {
                    self.op_expr(item)?;
                }
                self.line(&format!("local {name} = nil"));
                self.push(&name, Ty::OPS);
                return Ok(());
            }
            let (text, ty) = self.list(&items)?;
            self.line(&format!("local {name} = {text}"));
            self.push(&name, ty);
            return Ok(());
        }
        let (text, ty) = self.expr(&init.expr)?;
        self.line(&format!("local {name} = {text}"));
        self.push(&name, ty);
        Ok(())
    }

    fn list(&mut self, items: &[Expr]) -> Result<(String, Ty), Error> {
        if items.is_empty() {
            return Ok(("{}".to_owned(), Ty::LIST));
        }
        let mut values = Vec::new();
        for item in items {
            values.push(self.expr(item)?.0);
        }
        Ok((format!("{{ {} }}", values.join(", ")), Ty::LIST))
    }

    fn expr_stmt(&mut self, expr: &Expr) -> Result<(), Error> {
        match expr {
            Expr::Call(call) if is_ok_call(call) => self.ok_tail(&call.args),
            Expr::Call(call) if is_ops_push(call) => {
                let value = call.args.first().ok_or_else(|| {
                    Error::at(call.span(), "push() needs a value")
                })?;
                self.op_expr(value)
            }
            Expr::If(item) => self.if_stmt(item, false),
            Expr::Match(item) => self.match_stmt(item, false),
            Expr::ForLoop(item) => self.for_stmt(item),
            Expr::Assign(item) => self.assign(&item.left, &item.right, "="),
            Expr::Return(item) => self.ret(item.expr.as_deref()),
            Expr::Block(block) => {
                self.line("do");
                self.indent = self.indent.saturating_add(1);
                self.block_inner(&block.block, false)?;
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                Ok(())
            }
            Expr::Struct(_) => self.op_expr(expr),
            Expr::Macro(mac) if macro_name(mac) == "vec" => {
                for item in parse_vec(mac)? {
                    self.op_expr(&item)?;
                }
                Ok(())
            }
            Expr::Paren(paren) => self.expr_stmt(&paren.expr),
            Expr::Group(group) => self.expr_stmt(&group.expr),
            Expr::Tuple(tuple) if tuple.elems.is_empty() => Ok(()),
            _ => {
                let (text, ty) = self.expr(expr)?;
                if ty.base == Base::Void {
                    if !text.is_empty() {
                        self.line(&text);
                    }
                    Ok(())
                } else {
                    Err(Error::at(
                        expr.span(),
                        "this expression has no effect and is not a supported op",
                    ))
                }
            }
        }
    }
    fn ret(&mut self, expr: Option<&Expr>) -> Result<(), Error> {
        let Some(expr) = expr else {
            self.line("return");
            return Ok(());
        };
        if let Expr::Call(call) = expr {
            if is_ok_call(call) {
                return self.ok(&call.args);
            }
            if is_err_call(call) {
                let (text, _) =
                    self.expr(call.args.first().ok_or_else(|| {
                        Error::at(call.span(), "Err() needs a value")
                    })?)?;
                self.line(&format!("error({text})"));
                return Ok(());
            }
        }
        let (text, _) = self.expr(expr)?;
        self.line(&format!("return {text}"));
        Ok(())
    }

    fn ok(&mut self, args: &Punctuated<Expr, Token![,]>) -> Result<(), Error> {
        self.ok_impl(args, true)
    }

    fn ok_tail(
        &mut self,
        args: &Punctuated<Expr, Token![,]>,
    ) -> Result<(), Error> {
        self.ok_impl(args, false)
    }

    fn ok_impl(
        &mut self,
        args: &Punctuated<Expr, Token![,]>,
        emit_return: bool,
    ) -> Result<(), Error> {
        let value = args.first().ok_or_else(|| {
            Error::at(proc_macro2::Span::call_site(), "Ok() needs a value")
        })?;
        if let Expr::Call(call) = value
            && is_vec_new(call)
        {
            if emit_return {
                self.line("return");
            }
            return Ok(());
        }
        if let Expr::Macro(mac) = value
            && macro_name(mac) == "vec"
        {
            for item in parse_vec(mac)? {
                self.op_expr(&item)?;
            }
            if emit_return {
                self.line("return");
            }
            return Ok(());
        }
        if let Expr::Path(path) = value
            && let Some(segment) = path.path.get_ident()
            && self.lookup(&segment.to_string()) == Some(Ty::OPS)
        {
            if emit_return {
                self.line("return");
            }
            return Ok(());
        }
        Err(Error::at(
            value.span(),
            "Ok(...) must wrap Vec::new(), vec![...] or a local built from them",
        ))
    }

    fn if_stmt(&mut self, item: &syn::ExprIf, tail: bool) -> Result<(), Error> {
        let (chain, else_branch) = if_chain(item);
        if chain
            .iter()
            .all(|(condition, _)| self.const_bool(condition).is_some())
        {
            for (condition, block) in &chain {
                if self.const_bool(condition) == Some(true) {
                    return self.inline_block(block, tail);
                }
            }
            return self.emit_else(else_branch, tail);
        }
        self.write_if_chain(&chain, else_branch, tail, false)
    }

    fn emit_else(
        &mut self,
        else_branch: Option<&Expr>,
        tail: bool,
    ) -> Result<(), Error> {
        match else_branch {
            None => Ok(()),
            Some(Expr::Block(block)) => self.inline_block(&block.block, tail),
            Some(other) => Err(Error::at(
                other.span(),
                "only `else` and `else if` are supported",
            )),
        }
    }

    fn inline_block(
        &mut self,
        block: &syn::Block,
        tail: bool,
    ) -> Result<(), Error> {
        if tail {
            return self.block_inner(block, true);
        }
        self.line("do");
        self.indent = self.indent.saturating_add(1);
        self.block_inner(block, false)?;
        self.indent = self.indent.saturating_sub(1);
        self.line("end");
        Ok(())
    }

    fn write_if_chain(
        &mut self,
        chain: &[(&Expr, &syn::Block)],
        else_branch: Option<&Expr>,
        tail: bool,
        elseif: bool,
    ) -> Result<(), Error> {
        let Some((condition, block)) = chain.first().copied() else {
            return self.emit_else(else_branch, tail);
        };
        let (condition, _) = self.expr(condition)?;
        let keyword = if elseif { "elseif" } else { "if" };
        self.line(&format!("{keyword} {condition} then"));
        self.indent = self.indent.saturating_add(1);
        self.block_inner(block, tail)?;
        self.indent = self.indent.saturating_sub(1);
        if chain.len() > 1 {
            return self.write_if_chain(&chain[1..], else_branch, tail, true);
        }
        match else_branch {
            Some(Expr::Block(block)) => {
                self.line("else");
                self.indent = self.indent.saturating_add(1);
                self.block_inner(&block.block, tail)?;
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                Ok(())
            }
            None => {
                self.line("end");
                Ok(())
            }
            Some(other) => Err(Error::at(
                other.span(),
                "only `else` and `else if` are supported",
            )),
        }
    }

    fn const_bool(&self, expr: &Expr) -> Option<bool> {
        match expr {
            Expr::Lit(ExprLit {
                lit: Lit::Bool(value),
                ..
            }) => Some(value.value),
            Expr::Paren(paren) => self.const_bool(&paren.expr),
            Expr::Group(group) => self.const_bool(&group.expr),
            Expr::Reference(reference) => self.const_bool(&reference.expr),
            Expr::Unary(unary) if matches!(unary.op, UnOp::Not(_)) => {
                self.const_bool(&unary.expr).map(|value| !value)
            }
            Expr::Binary(binary) => {
                if matches!(binary.op, BinOp::And(_)) {
                    return Some(
                        self.const_bool(&binary.left)?
                            && self.const_bool(&binary.right)?,
                    );
                }
                if matches!(binary.op, BinOp::Or(_)) {
                    return Some(
                        self.const_bool(&binary.left)?
                            || self.const_bool(&binary.right)?,
                    );
                }
                let negate = matches!(binary.op, BinOp::Ne(_));
                if !negate && !matches!(binary.op, BinOp::Eq(_)) {
                    return None;
                }
                let left = self.const_literal(&binary.left);
                let right = self.const_literal(&binary.right);
                let value = match (
                    self.is_event_expr(&binary.left),
                    self.is_event_expr(&binary.right),
                ) {
                    (true, false) => {
                        self.bound_event.as_deref()? == right?.as_str()
                    }
                    (false, true) => {
                        self.bound_event.as_deref()? == left?.as_str()
                    }
                    (false, false) => left? == right?,
                    (true, true) => return None,
                };
                Some(if negate { !value } else { value })
            }
            Expr::MethodCall(call) if call.method == "is_load_event" => {
                match self.bound_event.as_deref() {
                    Some("load") => Some(true),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn const_literal(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Lit(ExprLit {
                lit: Lit::Str(value),
                ..
            }) => Some(value.value()),
            Expr::Lit(ExprLit {
                lit: Lit::Int(value),
                ..
            }) => Some(value.base10_digits().to_owned()),
            Expr::Lit(ExprLit {
                lit: Lit::Bool(value),
                ..
            }) => Some(if value.value { "true" } else { "false" }.to_owned()),
            Expr::Paren(paren) => self.const_literal(&paren.expr),
            Expr::Group(group) => self.const_literal(&group.expr),
            _ => None,
        }
    }

    fn match_stmt(
        &mut self,
        item: &syn::ExprMatch,
        tail: bool,
    ) -> Result<(), Error> {
        if let Some(bound) = self.bound_event.clone()
            && self.is_event_expr(&item.expr)
        {
            for arm in &item.arms {
                match pattern_strings(&arm.pat)? {
                    Some(patterns)
                        if patterns.iter().any(|pattern| pattern == &bound) =>
                    {
                        return self.inline_arm(&arm.body, tail);
                    }
                    Some(_) => {}
                    None if matches!(arm.pat, Pat::Wild(_)) => {
                        return self.inline_arm(&arm.body, tail);
                    }
                    None => {}
                }
            }
            return Ok(());
        }
        let (scrutinee, _) = self.expr(&item.expr)?;
        let event_match = self.is_event_expr(&item.expr);
        let mut opened = false;
        for arm in &item.arms {
            let patterns = pattern_strings(&arm.pat)?;
            let Some(patterns) = patterns else {
                if !matches!(arm.pat, Pat::Wild(_)) {
                    return Err(Error::at(
                        arm.pat.span(),
                        "only string or wildcard match patterns are supported",
                    ));
                }
                if opened {
                    self.line("else");
                }
                self.indent = self.indent.saturating_add(1);
                self.arm_body(&arm.body, tail)?;
                self.indent = self.indent.saturating_sub(1);
                continue;
            };
            if event_match {
                for pattern in &patterns {
                    self.events.insert(pattern.clone());
                }
            }
            let condition = patterns
                .iter()
                .map(|pattern| format!("{scrutinee} == {}", quote(pattern)))
                .collect::<Vec<_>>()
                .join(" or ");
            let keyword = if opened { "elseif" } else { "if" };
            self.line(&format!("{keyword} {condition} then"));
            self.indent = self.indent.saturating_add(1);
            self.arm_body(&arm.body, tail)?;
            self.indent = self.indent.saturating_sub(1);
            opened = true;
        }
        if opened {
            self.line("end");
        }
        Ok(())
    }

    fn inline_arm(&mut self, body: &Expr, tail: bool) -> Result<(), Error> {
        if tail {
            return self.arm_body(body, true);
        }
        self.line("do");
        self.indent = self.indent.saturating_add(1);
        self.arm_body(body, false)?;
        self.indent = self.indent.saturating_sub(1);
        self.line("end");
        Ok(())
    }

    fn arm_body(&mut self, body: &Expr, tail: bool) -> Result<(), Error> {
        match body {
            Expr::Block(block) => self.block_inner(&block.block, tail),
            other if tail => self.tail_expr(other),
            other => self.expr_stmt(other),
        }
    }

    fn for_stmt(&mut self, item: &syn::ExprForLoop) -> Result<(), Error> {
        let Pat::Ident(pattern) = &*item.pat else {
            return Err(Error::at(
                item.pat.span(),
                "only plain loop variables are supported",
            ));
        };
        let name = pattern.ident.to_string();
        let iterable = strip_reference(&item.expr);
        if let Expr::Range(range) = iterable {
            let start = match range.start.as_deref() {
                Some(start) => self.expr(start)?.0,
                None => "0".to_owned(),
            };
            let inclusive = matches!(range.limits, syn::RangeLimits::Closed(_));
            let Some(end) = range.end.as_deref() else {
                return Err(Error::at(
                    range.span(),
                    "open-ended ranges are not supported",
                ));
            };
            let end = self.expr(end)?.0;
            let end = if inclusive {
                end
            } else {
                format!("({end}) - 1")
            };
            self.push(&name, Ty::INT);
            self.line(&format!("for {name} = {start}, {end} do"));
            self.indent = self.indent.saturating_add(1);
            self.block(&item.body)?;
            self.indent = self.indent.saturating_sub(1);
            self.line("end");
            return Ok(());
        }
        let (text, ty) = self.expr(iterable)?;
        if ty.base != Base::List && ty.base != Base::Value {
            return Err(Error::at(
                item.expr.span(),
                "only ranges and list values can be looped over",
            ));
        }
        self.push(&name, Ty::VALUE);
        self.line(&format!("for _, {name} in ipairs({text}) do"));
        self.indent = self.indent.saturating_add(1);
        self.block(&item.body)?;
        self.indent = self.indent.saturating_sub(1);
        self.line("end");
        Ok(())
    }

    fn assign(
        &mut self,
        left: &Expr,
        right: &Expr,
        op: &str,
    ) -> Result<(), Error> {
        let Expr::Path(path) = strip_reference(left) else {
            return Err(Error::at(
                left.span(),
                "only assignments to local variables are supported",
            ));
        };
        let Some(name) = path.path.get_ident() else {
            return Err(Error::at(
                left.span(),
                "only assignments to local variables are supported",
            ));
        };
        let name = name.to_string();
        let (text, ty) = self.expr(right)?;
        self.line(&format!("{name} {op} {text}"));
        self.push(&name, ty);
        Ok(())
    }

    fn expr(&mut self, expr: &Expr) -> Result<(String, Ty), Error> {
        match expr {
            Expr::Lit(lit) => self.literal(lit),
            Expr::Path(path) => self.path(expr, path),
            Expr::Paren(paren) => self.expr(&paren.expr),
            Expr::Group(group) => self.expr(&group.expr),
            Expr::Reference(reference) => self.expr(&reference.expr),
            Expr::Unary(unary) => self.unary(unary),
            Expr::Binary(binary) => self.binary(binary),
            Expr::MethodCall(call) => self.method(call),
            Expr::Call(call) => self.call(call),
            Expr::Field(field) => self.field(expr, field),
            Expr::Macro(mac) => self.macro_expr(mac),
            Expr::Index(index) => self.index(index),
            Expr::Cast(cast) => self.cast(cast),
            Expr::Struct(_) => {
                self.op_expr(expr)?;
                Ok((String::new(), Ty::VOID))
            }
            Expr::If(item) => {
                let (chain, else_branch) = if_chain(item);
                if chain
                    .iter()
                    .all(|(condition, _)| self.const_bool(condition).is_some())
                {
                    for (condition, block) in &chain {
                        if self.const_bool(condition) == Some(true) {
                            return self.block_value(block);
                        }
                    }
                    return match else_branch {
                        Some(Expr::Block(block)) => {
                            self.block_value(&block.block)
                        }
                        _ => Ok(("nil".to_owned(), Ty::VALUE)),
                    };
                }
                let name = self.temp();
                self.line(&format!("local {name}"));
                let (condition, _) = self.expr(&item.cond)?;
                self.line(&format!("if {condition} then"));
                self.indent = self.indent.saturating_add(1);
                let (value, ty) = self.block_value(&item.then_branch)?;
                self.line(&format!("{name} = {value}"));
                self.indent = self.indent.saturating_sub(1);
                if let Some((_, branch)) = &item.else_branch {
                    self.line("else");
                    self.indent = self.indent.saturating_add(1);
                    let (value, _) = match &**branch {
                        Expr::Block(block) => self.block_value(&block.block)?,
                        other => self.expr(other)?,
                    };
                    self.line(&format!("{name} = {value}"));
                    self.indent = self.indent.saturating_sub(1);
                }
                self.line("end");
                Ok((name, ty))
            }
            Expr::Array(array) => {
                let mut values = Vec::new();
                for item in &array.elems {
                    values.push(self.expr(item)?.0);
                }
                Ok((format!("{{ {} }}", values.join(", ")), Ty::LIST))
            }
            Expr::Tuple(tuple) if tuple.elems.is_empty() => {
                Ok(("nil".to_owned(), Ty::VOID))
            }
            other => Err(Error::at(
                other.span(),
                "unsupported expression in a compilable handler",
            )),
        }
    }

    fn temp(&mut self) -> String {
        self.temp = self.temp.saturating_add(1);
        format!("__t{}", self.temp)
    }

    fn block_value(
        &mut self,
        block: &syn::Block,
    ) -> Result<(String, Ty), Error> {
        match block.stmts.last() {
            Some(Stmt::Expr(value, _)) => self.expr(value),
            _ => Ok(("nil".to_owned(), Ty::VALUE)),
        }
    }

    fn literal(&mut self, lit: &ExprLit) -> Result<(String, Ty), Error> {
        match &lit.lit {
            Lit::Str(value) => Ok((quote(&value.value()), Ty::STR)),
            Lit::Int(value) => Ok((value.base10_digits().to_owned(), Ty::INT)),
            Lit::Float(value) => {
                Ok((value.base10_digits().to_owned(), Ty::FLOAT))
            }
            Lit::Bool(value) => Ok((
                if value.value { "true" } else { "false" }.to_owned(),
                Ty::BOOL,
            )),
            Lit::Char(value) => {
                Ok((quote(&value.value().to_string()), Ty::STR))
            }
            other => Err(Error::at(other.span(), "unsupported literal")),
        }
    }

    fn path(
        &mut self,
        expr: &Expr,
        path: &syn::ExprPath,
    ) -> Result<(String, Ty), Error> {
        let Some(name) = path.path.get_ident() else {
            return Err(Error::at(expr.span(), "unsupported path expression"));
        };
        let name = name.to_string();
        if name == "true" || name == "false" {
            return Ok((name, Ty::BOOL));
        }
        if name == "None" {
            return Ok(("nil".to_owned(), Ty::VALUE));
        }
        if name == self.ctx {
            return Err(Error::at(
                expr.span(),
                "the context must be accessed through a supported field or method",
            ));
        }
        if let Some(ty) = self.lookup(&name) {
            return Ok((name, ty));
        }
        if let Some((value, ty)) = self.constants.get(&name) {
            return Ok((value.clone(), *ty));
        }
        Err(Error::at(
            expr.span(),
            format!("unknown variable `{name}` in a compilable handler"),
        ))
    }

    fn unary(&mut self, unary: &syn::ExprUnary) -> Result<(String, Ty), Error> {
        let (text, ty) = self.expr(&unary.expr)?;
        match unary.op {
            UnOp::Not(_) => Ok((format!("(not {text})"), Ty::BOOL)),
            UnOp::Neg(_) => Ok((format!("(-{text})"), ty)),
            other => Err(Error::at(other.span(), "unsupported unary operator")),
        }
    }

    fn binary(&mut self, binary: &ExprBinary) -> Result<(String, Ty), Error> {
        if let Some(operator) = compound_operator(&binary.op) {
            self.assign(&binary.left, &binary.right, operator)?;
            return Ok((String::new(), Ty::VOID));
        }
        if matches!(binary.op, BinOp::Eq(_)) {
            self.record_event(&binary.left, &binary.right);
            self.record_event(&binary.right, &binary.left);
        }
        let (left, left_ty) = self.expr(&binary.left)?;
        let (right, right_ty) = self.expr(&binary.right)?;
        if let Some(operator) = compare_operator(&binary.op) {
            return Ok((format!("({left} {operator} {right})"), Ty::BOOL));
        }
        let integer = left_ty.base == Base::Int && right_ty.base == Base::Int;
        match binary.op {
            BinOp::Add(_) => Ok((
                format!("({left} + {right})"),
                numeric(&left_ty, &right_ty),
            )),
            BinOp::Sub(_) => Ok((
                format!("({left} - {right})"),
                numeric(&left_ty, &right_ty),
            )),
            BinOp::Mul(_) => Ok((
                format!("({left} * {right})"),
                numeric(&left_ty, &right_ty),
            )),
            BinOp::Div(_) if integer => {
                Ok((format!("(math.modf({left} / {right}))"), Ty::INT))
            }
            BinOp::Div(_) => Ok((
                format!("({left} / {right})"),
                numeric(&left_ty, &right_ty),
            )),
            BinOp::Rem(_) if integer => {
                self.uses_irem = true;
                Ok((format!("irem({left}, {right})"), Ty::INT))
            }
            BinOp::Rem(_) => Ok((
                format!("({left} % {right})"),
                numeric(&left_ty, &right_ty),
            )),
            other => {
                Err(Error::at(other.span(), "unsupported binary operator"))
            }
        }
    }

    fn method(&mut self, call: &ExprMethodCall) -> Result<(String, Ty), Error> {
        let method = call.method.to_string();
        let args = call.args.iter().collect::<Vec<_>>();
        if self.is_ctx_expr(&call.receiver) {
            return self.context_method(call, &method, &args);
        }
        let (receiver, ty) = self.expr(&call.receiver)?;
        match method.as_str() {
            "to_string" => Ok((format!("tostring({receiver})"), Ty::STR)),
            "into_owned" | "to_owned" | "clone" | "as_ref" => {
                Ok((receiver, ty))
            }
            "as_str" => Ok((receiver, Ty::OPT_STR)),
            "as_i64" | "as_u64" => Ok((receiver, Ty::OPT_INT)),
            "as_f64" => Ok((receiver, Ty::OPT_FLOAT)),
            "as_bool" => Ok((receiver, Ty::OPT_BOOL)),
            "as_array" => Ok((receiver, Ty::OPT_LIST)),
            "is_some" => Ok((format!("({receiver} ~= nil)"), Ty::BOOL)),
            "is_none" => Ok((format!("({receiver} == nil)"), Ty::BOOL)),
            "unwrap" => {
                self.uses_unwrap = true;
                Ok((format!("unwrap({receiver})"), Ty::new(ty.base)))
            }
            "unwrap_or" => {
                let (default, default_ty) = self.expr(arg(&args, 0, call)?)?;
                if ty.base == Base::Bool || default_ty.base == Base::Bool {
                    self.uses_bool_or = true;
                    return Ok((
                        format!("bool_or({receiver}, {default})"),
                        Ty::BOOL,
                    ));
                }
                Ok((
                    format!("({receiver} or {default})"),
                    Ty::new(wider(ty, default_ty).base),
                ))
            }
            "unwrap_or_default" => {
                let Some(default) = ty.default_literal() else {
                    return Err(Error::at(
                        call.span(),
                        "this type has no default in a compilable handler",
                    ));
                };
                Ok((format!("({receiver} or {default})"), Ty::new(ty.base)))
            }
            "len" => Ok((format!("#{receiver}"), Ty::INT)),
            "is_empty" => Ok((format!("(#{receiver} == 0)"), Ty::BOOL)),
            "get" => {
                let (key, key_ty) = self.expr(arg(&args, 0, call)?)?;
                if key_ty.base == Base::Int {
                    Ok((format!("{receiver}[({key}) + 1]"), Ty::OPT_VALUE))
                } else {
                    Ok((format!("{receiver}[{key}]"), Ty::OPT_VALUE))
                }
            }
            "trim" => Ok((format!("string.trim({receiver})"), Ty::STR)),
            "to_lowercase" => {
                Ok((format!("string.lower({receiver})"), Ty::STR))
            }
            "to_uppercase" => {
                Ok((format!("string.upper({receiver})"), Ty::STR))
            }
            "contains" => {
                let (needle, _) = self.expr(arg(&args, 0, call)?)?;
                Ok((
                    format!(
                        "(string.find({receiver}, {needle}, 1, true) ~= nil)"
                    ),
                    Ty::BOOL,
                ))
            }
            "push" if ty.base == Base::Ops => {
                self.op_expr(arg(&args, 0, call)?)?;
                Ok((String::new(), Ty::VOID))
            }
            "push" => {
                let (value, _) = self.expr(arg(&args, 0, call)?)?;
                Ok((format!("table.insert({receiver}, {value})"), Ty::VOID))
            }
            _ => Err(Error::at(
                call.method.span(),
                format!(
                    "method `{method}` is not supported by the Luau compiler"
                ),
            )),
        }
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
                let path = self.string_arg(args, 0, call)?;
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
                let key = self.string_arg(args, 0, call)?;
                let params = arg(args, 1, call)?;
                let params = strip_reference(params);
                let table = self.tr_params(params)?;
                Ok((format!("noctalia.tr({key}, {table})"), Ty::STR))
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
            other => Err(Error::at(
                call.method.span(),
                format!(
                    "context method `{other}` is not supported by the Luau compiler"
                ),
            )),
        }
    }

    fn coerced_string(&mut self, value: String, allow_bool: bool) -> String {
        let name = self.temp();
        self.line(&format!("local {name} = {value}"));
        if allow_bool {
            format!(
                "((type({name}) == \"string\" and {name}) or ((type({name}) == \"number\" or type({name}) == \"boolean\") and tostring({name})))"
            )
        } else {
            format!(
                "((type({name}) == \"string\" and {name}) or (type({name}) == \"number\" and tostring({name})))"
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

    fn string_arg(
        &mut self,
        args: &[&Expr],
        index: usize,
        call: &ExprMethodCall,
    ) -> Result<String, Error> {
        let value = arg(args, index, call)?;
        let (text, ty) = self.expr(value)?;
        if ty.base != Base::Str {
            return Err(Error::at(
                value.span(),
                "expected a string literal argument",
            ));
        }
        Ok(text)
    }

    fn call(&mut self, call: &ExprCall) -> Result<(String, Ty), Error> {
        if let Some(name) = path_last_name(&call.func) {
            let args = call.args.iter().collect::<Vec<_>>();
            match name.as_str() {
                "Some" => return self.expr(arg(&args, 0, call)?),
                "String::new" => return Ok(("\"\"".to_owned(), Ty::STR)),
                "Vec::new" => return Ok(("{}".to_owned(), Ty::LIST)),
                _ => {}
            }
            if let Some(kind) = ui_constructor(&name) {
                return self.ui_node(call, kind);
            }
            if let Some(op) = op_constructor(&name) {
                self.op_call(call, &op)?;
                return Ok((String::new(), Ty::VOID));
            }
        }
        Err(Error::at(
            call.span(),
            "only ops, ui constructors and a small set of std calls are supported",
        ))
    }

    fn ui_node(
        &mut self,
        call: &ExprCall,
        kind: &str,
    ) -> Result<(String, Ty), Error> {
        let args = call.args.iter().collect::<Vec<_>>();
        let props = match args.first() {
            Some(expr) => self.expr(expr)?.0,
            None => "{}".to_owned(),
        };
        let children = match args.get(1) {
            Some(expr) => {
                let expr = strip_reference(expr);
                if let Expr::Call(vec) = expr
                    && is_vec_new(vec)
                {
                    "{}".to_owned()
                } else if let Expr::Macro(mac) = expr
                    && macro_name(mac) == "vec"
                {
                    let items = parse_vec(mac)?;
                    self.list(&items)?.0
                } else {
                    self.expr(expr)?.0
                }
            }
            None => "{}".to_owned(),
        };
        Ok((
            format!(
                "{{ kind = {}, props = {props}, children = {children} }}",
                quote(kind)
            ),
            Ty::NODE,
        ))
    }

    fn field(
        &mut self,
        expr: &Expr,
        field: &syn::ExprField,
    ) -> Result<(String, Ty), Error> {
        let Member::Named(name) = &field.member else {
            return Err(Error::at(
                expr.span(),
                "tuple field access is not supported",
            ));
        };
        if self.is_ctx_expr(&field.base) {
            return match name.to_string().as_str() {
                "event" => Ok(match &self.bound_event {
                    Some(event) => (quote(event), Ty::STR),
                    None => ("event".to_owned(), Ty::STR),
                }),
                "now_ms" => Ok(("noctalia.nowMs()".to_owned(), Ty::INT)),
                "entry" => {
                    self.uses_entry = true;
                    Ok(("entry".to_owned(), Ty::STR))
                }
                "payload" => {
                    self.uses_payload = true;
                    self.payload_escapes = true;
                    Ok(("payload".to_owned(), Ty::VALUE))
                }
                "state" => {
                    self.uses_state = true;
                    Ok(("plugin_state".to_owned(), Ty::VALUE))
                }
                "data_dir" => Ok((
                    "(noctalia.pluginDataDir() or noctalia.pluginDir())"
                        .to_owned(),
                    Ty::STR,
                )),
                other => Err(Error::at(
                    field.member.span(),
                    format!(
                        "context field `{other}` is not supported by the Luau compiler"
                    ),
                )),
            };
        }
        let (base, _) = self.expr(&field.base)?;
        Ok((
            format!("{base}[{}]", lua_key(&name.to_string())),
            Ty::OPT_VALUE,
        ))
    }

    fn index(&mut self, index: &syn::ExprIndex) -> Result<(String, Ty), Error> {
        let (base, _) = self.expr(&index.expr)?;
        let (key, key_ty) = self.expr(&index.index)?;
        if key_ty.base == Base::Int {
            Ok((format!("{base}[({key}) + 1]"), Ty::OPT_VALUE))
        } else {
            Ok((format!("{base}[{key}]"), Ty::OPT_VALUE))
        }
    }

    fn cast(&mut self, cast: &syn::ExprCast) -> Result<(String, Ty), Error> {
        let (text, _) = self.expr(&cast.expr)?;
        Ok((text, type_hint(&cast.ty)))
    }

    fn macro_expr(&mut self, mac: &ExprMacro) -> Result<(String, Ty), Error> {
        match macro_name(mac).as_str() {
            "format" => self.format(mac),
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

    fn format(&mut self, mac: &ExprMacro) -> Result<(String, Ty), Error> {
        let parsed: FormatArgs = syn::parse2(mac.mac.tokens.clone())?;
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
                    if placeholder.contains(':') {
                        return Err(Error::at(
                            mac.span(),
                            "format specifiers are not supported by the Luau compiler",
                        ));
                    }
                    if placeholder.is_empty() {
                        let (_, expr) = positional.next().ok_or_else(|| {
                            Error::at(
                                mac.span(),
                                "format! has more placeholders than arguments",
                            )
                        })?;
                        let (value, ty) = self.expr(expr)?;
                        pieces.push(format_piece(value, ty));
                    } else if let Some((_, expr)) = parsed
                        .arguments
                        .iter()
                        .find(|(key, _)| key.as_deref() == Some(&placeholder))
                    {
                        let (value, ty) = self.expr(expr)?;
                        pieces.push(format_piece(value, ty));
                    } else {
                        let ty = self.lookup(&placeholder).ok_or_else(|| {
                            Error::at(
                                mac.span(),
                                format!(
                                    "format! placeholder `{placeholder}` has no matching argument"
                                ),
                            )
                        })?;
                        pieces.push(format_piece(placeholder, ty));
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
        Ok((pieces.join(" .. "), Ty::STR))
    }

    fn json(&mut self, mac: &ExprMacro) -> Result<(String, Ty), Error> {
        let node: JsonNode = syn::parse2(mac.mac.tokens.clone())?;
        self.json_node(&node)
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
                Ok((format!("{{ {} }}", parts.join(", ")), Ty::TABLE))
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
                Ok((format!("{{ {} }}", parts.join(", ")), Ty::LIST))
            }
            JsonNode::Expr(expr) => {
                if let Expr::Path(path) = &**expr
                    && path.path.is_ident("null")
                {
                    return Ok(("nil".to_owned(), Ty::VALUE));
                }
                self.expr(expr)
            }
        }
    }

    fn record_event(&mut self, event: &Expr, literal: &Expr) {
        if !self.is_event_expr(event) {
            return;
        }
        if let Expr::Lit(ExprLit {
            lit: Lit::Str(value),
            ..
        }) = literal
        {
            self.events.insert(value.value());
        }
    }

    fn is_ctx_expr(&self, expr: &Expr) -> bool {
        matches!(expr, Expr::Path(path) if path.path.is_ident(&self.ctx))
    }

    fn is_event_expr(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Field(field) => {
                self.is_ctx_expr(&field.base)
                    && matches!(&field.member, Member::Named(name) if name == "event")
            }
            Expr::MethodCall(call) => {
                self.is_event_expr(&call.receiver) && call.method == "as_str"
            }
            Expr::Paren(paren) => self.is_event_expr(&paren.expr),
            Expr::Group(group) => self.is_event_expr(&group.expr),
            _ => false,
        }
    }

    fn op_expr(&mut self, expr: &Expr) -> Result<(), Error> {
        self.emitted = true;
        match expr {
            Expr::Struct(item) => {
                let segments = item
                    .path
                    .segments
                    .iter()
                    .map(|segment| segment.ident.to_string())
                    .collect::<Vec<_>>();
                if segments.len() < 2
                    || segments.get(segments.len().saturating_sub(2))
                        != Some(&"Op".to_owned())
                {
                    return Err(Error::at(
                        item.span(),
                        "expected an Op constructor",
                    ));
                }
                let name = segments.last().cloned().unwrap_or_default();
                self.op_struct(&name, item)
            }
            Expr::Call(call) => {
                let name = path_last_name(&call.func).ok_or_else(|| {
                    Error::at(call.span(), "unsupported op expression")
                })?;
                let variant =
                    name.rsplit("::").next().unwrap_or(&name).to_owned();
                if op_constructor(&name).is_some() {
                    return self.op_call(call, &variant);
                }
                Err(Error::at(call.span(), "expected an Op constructor"))
            }
            Expr::Paren(paren) => self.op_expr(&paren.expr),
            Expr::Group(group) => self.op_expr(&group.expr),
            Expr::Path(path) => {
                let segments = path
                    .path
                    .segments
                    .iter()
                    .map(|segment| segment.ident.to_string())
                    .collect::<Vec<_>>();
                let name = if segments.len() >= 2
                    && segments.get(segments.len().saturating_sub(2))
                        == Some(&"Op".to_owned())
                {
                    segments.last().cloned().unwrap_or_default()
                } else {
                    String::new()
                };
                if name == "OpenSettings" {
                    self.line("noctalia.openSettings()");
                    Ok(())
                } else {
                    Err(Error::at(
                        expr.span(),
                        format!("op `{name}` is not supported"),
                    ))
                }
            }
            other => Err(Error::at(other.span(), "expected an Op constructor")),
        }
    }

    fn op_struct(
        &mut self,
        name: &str,
        item: &syn::ExprStruct,
    ) -> Result<(), Error> {
        let mut fields = HashMap::new();
        for field in &item.fields {
            let Member::Named(member) = &field.member else {
                return Err(Error::at(
                    field.member.span(),
                    "unsupported op field",
                ));
            };
            fields.insert(member.to_string(), &field.expr);
        }
        let field = |name: &str| -> Result<&Expr, Error> {
            fields.get(name).copied().ok_or_else(|| {
                Error::at(
                    item.span(),
                    format!("op `{name}` is missing its `{name}` field"),
                )
            })
        };
        match name {
            "Log" => {
                let message = self.expr(field("message")?)?.0;
                self.line(&format!("noctalia.log({message})"));
            }
            "Notify" | "NotifyError" => {
                let title = self.expr(field("title")?)?.0;
                let body = self.expr(field("body")?)?.0;
                let options = match fields.get("options") {
                    Some(expr) => self.expr(expr)?.0,
                    None => "nil".to_owned(),
                };
                let function = if name == "Notify" {
                    "noctalia.notify"
                } else {
                    "noctalia.notifyError"
                };
                self.line(&format!("{function}({title}, {body}, {options})"));
            }
            "SetInterval" => {
                let ms = self.expr(field("ms")?)?.0;
                self.line(&format!("noctalia.setUpdateInterval({ms})"));
            }
            "StateSet" => {
                let key = self.expr(field("key")?)?.0;
                let value = self.expr(field("value")?)?.0;
                self.line(&format!("state.set({key}, {value})"));
            }
            "TogglePanel" => {
                let id = self.expr(field("id")?)?.0;
                self.line(&format!("noctalia.togglePanel({id})"));
            }
            "OpenSettings" => self.line("noctalia.openSettings()"),
            "BarText" => {
                let text = self.expr(field("text")?)?.0;
                self.line(&format!("barWidget.setText({text})"));
            }
            "BarColor" => {
                let role = self.expr(field("role")?)?.0;
                self.line(&format!("barWidget.setColor({role})"));
            }
            "BarVisible" => {
                let visible = self.expr(field("visible")?)?.0;
                self.line(&format!("barWidget.setVisible({visible})"));
            }
            "BarTooltip" => {
                let value = self.expr(field("value")?)?.0;
                self.line(&format!("barWidget.setTooltip({value})"));
            }
            "BarImage" => {
                let path = self.expr(field("path")?)?.0;
                let preserve = self.expr(field("preserve")?)?.0;
                let width = self.expr(field("width")?)?.0;
                let height = self.expr(field("height")?)?.0;
                self.line(&format!(
                    "barWidget.setImage({path}, {preserve}, {width}, {height})"
                ));
            }
            "BarRender" => {
                let tree = self.expr(field("tree")?)?.0;
                self.line(&format!("barWidget.render({tree})"));
            }
            "PanelRender" => {
                let tree = self.expr(field("tree")?)?.0;
                self.line(&format!("panel.render({tree})"));
            }
            "DesktopWidgetRender" => {
                let tree = self.expr(field("tree")?)?.0;
                self.line(&format!("desktopWidget.render({tree})"));
            }
            "LayerShow" => {
                let id = self.expr(field("id")?)?.0;
                self.line(&format!("noctalia.layer.show({id})"));
            }
            "LayerHide" => {
                let id = self.expr(field("id")?)?.0;
                self.line(&format!("noctalia.layer.hide({id})"));
            }
            "LayerRender" => {
                let tree = self.expr(field("tree")?)?.0;
                self.line(&format!("layer.render({tree})"));
            }
            "LayerConfigure" => {
                let id = self.expr(field("id")?)?.0;
                let props = self.expr(field("props")?)?.0;
                self.line(&format!("noctalia.layer.configure({id}, {props})"));
            }
            "LauncherResults" => {
                let text = self.expr(field("text")?)?.0;
                let rows = self.expr(field("rows")?)?.0;
                self.line(&format!("launcher.setResults({text}, {rows})"));
            }
            "RemoveFile" => {
                let path = self.expr(field("path")?)?.0;
                self.line(&format!("noctalia.removeFile({path})"));
            }
            "HostCall" => {
                self.uses_host_call = true;
                let root = self.expr(field("root")?)?.0;
                let path = self.expr(field("path")?)?.0;
                let args = self.expr(field("args")?)?.0;
                self.line(&format!("host_call({root}, {path}, {args})"));
            }
            other => {
                return Err(Error::at(
                    item.span(),
                    format!(
                        "op `{other}` is not supported by the Luau compiler"
                    ),
                ));
            }
        }
        Ok(())
    }

    fn op_call(&mut self, call: &ExprCall, name: &str) -> Result<(), Error> {
        let args = call.args.iter().collect::<Vec<_>>();
        match name {
            "log" | "log_static" => {
                let message = self.expr(arg(&args, 0, call)?)?.0;
                self.line(&format!("noctalia.log({message})"));
            }
            "state" => {
                let key = self.expr(arg(&args, 0, call)?)?.0;
                let value = self.expr(arg(&args, 1, call)?)?.0;
                self.line(&format!("state.set({key}, {value})"));
            }
            "state_watch" => {
                let key = self.expr(arg(&args, 0, call)?)?.0;
                let id = self.expr(arg(&args, 1, call)?)?.0;
                self.line(&format!(
                    "state.watch({key}, {{ __host_callback = {id} }})"
                ));
            }
            "set_interval" => {
                let ms = self.expr(arg(&args, 0, call)?)?.0;
                self.line(&format!("noctalia.setUpdateInterval({ms})"));
            }
            "host_call" => {
                self.uses_host_call = true;
                let root = self.expr(arg(&args, 0, call)?)?.0;
                let path = self.expr(arg(&args, 1, call)?)?.0;
                let value = self.expr(arg(&args, 2, call)?)?.0;
                self.line(&format!("host_call({root}, {path}, {value})"));
            }
            "notify" | "notify_with_options" => {
                let title = self.expr(arg(&args, 0, call)?)?.0;
                let body = self.expr(arg(&args, 1, call)?)?.0;
                let options = if name == "notify_with_options" {
                    self.expr(arg(&args, 2, call)?)?.0
                } else {
                    "nil".to_owned()
                };
                self.line(&format!(
                    "noctalia.notify({title}, {body}, {options})"
                ));
            }
            "notify_error" | "notify_error_with_options" => {
                let title = self.expr(arg(&args, 0, call)?)?.0;
                let body = self.expr(arg(&args, 1, call)?)?.0;
                let options = if name == "notify_error_with_options" {
                    self.expr(arg(&args, 2, call)?)?.0
                } else {
                    "nil".to_owned()
                };
                self.line(&format!(
                    "noctalia.notifyError({title}, {body}, {options})"
                ));
            }
            "copy" => {
                let text = self.expr(arg(&args, 0, call)?)?.0;
                self.line(&format!("noctalia.copyToClipboard({text})"));
            }
            "copy_with_mime" => {
                let text = self.expr(arg(&args, 0, call)?)?.0;
                let mime = self.expr(arg(&args, 1, call)?)?.0;
                self.line(&format!("noctalia.copyToClipboard({text}, {mime})"));
            }
            "open_path" => {
                let path = self.expr(arg(&args, 0, call)?)?.0;
                self.line(&format!("noctalia.openPath({path})"));
            }
            other => {
                return Err(Error::at(
                    call.span(),
                    format!("op constructor `{other}` is not supported"),
                ));
            }
        }
        Ok(())
    }
}

fn compare_operator(operator: &BinOp) -> Option<&'static str> {
    match operator {
        BinOp::Eq(_) => Some("=="),
        BinOp::Ne(_) => Some("~="),
        BinOp::Lt(_) => Some("<"),
        BinOp::Le(_) => Some("<="),
        BinOp::Gt(_) => Some(">"),
        BinOp::Ge(_) => Some(">="),
        BinOp::And(_) => Some("and"),
        BinOp::Or(_) => Some("or"),
        _ => None,
    }
}

fn compound_operator(operator: &BinOp) -> Option<&'static str> {
    match operator {
        BinOp::AddAssign(_) => Some("+="),
        BinOp::SubAssign(_) => Some("-="),
        BinOp::MulAssign(_) => Some("*="),
        _ => None,
    }
}

fn format_piece(value: String, ty: Ty) -> String {
    if ty.is_string() || ty.is_numeric() {
        value
    } else {
        format!("tostring({value})")
    }
}

fn numeric(left: &Ty, right: &Ty) -> Ty {
    if left.base == Base::Float || right.base == Base::Float {
        Ty::FLOAT
    } else {
        Ty::INT
    }
}

fn wider(left: Ty, right: Ty) -> Ty {
    if left.base == right.base {
        left
    } else if left.is_numeric() && right.is_numeric() {
        Ty::FLOAT
    } else {
        Ty::VALUE
    }
}

fn type_hint(ty: &Type) -> Ty {
    let text = type_text(ty);
    if text.contains("String") || text == "str" {
        Ty::STR
    } else if text == "bool" {
        Ty::BOOL
    } else if text.starts_with("Vec") || text.starts_with("Option<Vec") {
        Ty::LIST
    } else if matches!(text.as_str(), "i64" | "u64" | "usize" | "u32" | "i32") {
        Ty::INT
    } else if matches!(text.as_str(), "f64" | "f32") {
        Ty::FLOAT
    } else if text == "Op" {
        Ty::OPS
    } else {
        Ty::VALUE
    }
}

fn type_text(ty: &Type) -> String {
    match ty {
        Type::Path(path) => path
            .path
            .segments
            .last()
            .map_or_else(String::new, |segment| segment.ident.to_string()),
        Type::Reference(reference) => type_text(&reference.elem),
        _ => String::new(),
    }
}

fn turbofish_type(
    arguments: Option<&syn::AngleBracketedGenericArguments>,
) -> Ty {
    let Some(arguments) = arguments else {
        return Ty::OPT_VALUE;
    };
    let Some(syn::GenericArgument::Type(ty)) = arguments.args.first() else {
        return Ty::OPT_VALUE;
    };
    let text = type_text(ty);
    let base = if text == "String" || text == "str" {
        Base::Str
    } else if text == "bool" {
        Base::Bool
    } else if matches!(text.as_str(), "i64" | "u64" | "usize") {
        Base::Int
    } else if matches!(text.as_str(), "f64" | "f32") {
        Base::Float
    } else if text.starts_with("Vec") {
        Base::List
    } else {
        Base::Value
    };
    Ty::opt(base)
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

fn qualified_variant<'a>(name: &'a str, root: &str) -> Option<&'a str> {
    let mut parts = name.rsplit("::");
    let variant = parts.next()?;
    (parts.next()? == root).then_some(variant)
}

fn op_constructor(name: &str) -> Option<String> {
    let variant = qualified_variant(name, "Op")?;
    let supported = [
        "log",
        "log_static",
        "state",
        "state_watch",
        "set_interval",
        "host_call",
        "notify",
        "notify_with_options",
        "notify_error",
        "notify_error_with_options",
        "copy",
        "copy_with_mime",
        "open_path",
    ];
    supported.contains(&variant).then(|| variant.to_owned())
}

fn ui_constructor(name: &str) -> Option<&'static str> {
    let variant = qualified_variant(name, "ui")?;
    match variant {
        "column" => Some("column"),
        "row" => Some("row"),
        "scroll" => Some("scroll"),
        "label" => Some("label"),
        "button" => Some("button"),
        "input" => Some("input"),
        "progress" => Some("progress"),
        _ => None,
    }
}

fn path_last_name(expr: &Expr) -> Option<String> {
    let Expr::Path(path) = expr else {
        return None;
    };
    let segments = path
        .path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>();
    if segments.is_empty() {
        None
    } else {
        Some(segments.join("::"))
    }
}

fn macro_name(mac: &ExprMacro) -> String {
    mac.mac
        .path
        .segments
        .last()
        .map_or_else(String::new, |segment| segment.ident.to_string())
}

fn is_ok_call(call: &ExprCall) -> bool {
    path_last_name(&call.func).as_deref() == Some("Ok")
}

fn is_err_call(call: &ExprCall) -> bool {
    path_last_name(&call.func).as_deref() == Some("Err")
}

fn is_vec_new(call: &ExprCall) -> bool {
    path_last_name(&call.func).as_deref() == Some("Vec::new")
}

fn is_ops_push(call: &ExprCall) -> bool {
    let Expr::MethodCall(method) = &*call.func else {
        return false;
    };
    method.method == "push" && call.args.len() == 1
}

fn is_op_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Struct(item) => {
            let segments = item.path.segments.iter().collect::<Vec<_>>();
            segments.len() >= 2
                && segments
                    .get(segments.len().saturating_sub(2))
                    .is_some_and(|segment| segment.ident == "Op")
        }
        Expr::Call(call) => path_last_name(&call.func)
            .is_some_and(|name| op_constructor(&name).is_some()),
        _ => false,
    }
}

fn arg<'a>(
    args: &[&'a Expr],
    index: usize,
    call: &impl Spanned,
) -> Result<&'a Expr, Error> {
    args.get(index).copied().ok_or_else(|| {
        Error::at(call.span(), format!("missing argument {index}"))
    })
}

fn expr_string(expr: &Expr) -> Result<String, Error> {
    let Expr::Lit(ExprLit {
        lit: Lit::Str(value),
        ..
    }) = expr
    else {
        return Err(Error::at(expr.span(), "expected a string literal"));
    };
    Ok(value.value())
}

fn unquote(value: &str) -> String {
    value.trim_matches('"').to_owned()
}

fn lua_key(key: &str) -> String {
    if is_identifier(key) {
        key.to_owned()
    } else {
        format!("[{}]", quote(key))
    }
}

fn strip_reference(expr: &Expr) -> &Expr {
    match expr {
        Expr::Reference(reference) => strip_reference(&reference.expr),
        Expr::Paren(paren) => strip_reference(&paren.expr),
        Expr::Group(group) => strip_reference(&group.expr),
        other => other,
    }
}

type IfChain<'a> = (Vec<(&'a Expr, &'a syn::Block)>, Option<&'a Expr>);

fn diverges(expr: &Expr) -> bool {
    match expr {
        Expr::Return(_) => true,
        Expr::Call(call) => is_ok_call(call),
        Expr::Paren(paren) => diverges(&paren.expr),
        Expr::Group(group) => diverges(&group.expr),
        _ => false,
    }
}

fn if_chain(item: &syn::ExprIf) -> IfChain<'_> {
    let mut chain = Vec::new();
    let mut current = item;
    loop {
        chain.push((&*current.cond, &current.then_branch));
        let next = current.else_branch.as_ref().map(|(_, expr)| &**expr);
        match next {
            Some(Expr::If(nested)) => current = nested,
            other => return (chain, other),
        }
    }
}

fn pattern_strings(pattern: &Pat) -> Result<Option<Vec<String>>, Error> {
    match pattern {
        Pat::Lit(literal) => {
            let Lit::Str(value) = &literal.lit else {
                return Ok(None);
            };
            Ok(Some(vec![value.value()]))
        }
        Pat::Or(or) => {
            let mut values = Vec::new();
            for case in &or.cases {
                let Some(mut item) = pattern_strings(case)? else {
                    return Ok(None);
                };
                values.append(&mut item);
            }
            Ok(Some(values))
        }
        Pat::Paren(paren) => pattern_strings(&paren.pat),
        _ => Ok(None),
    }
}

fn parse_vec(mac: &ExprMacro) -> Result<Vec<Expr>, Error> {
    let list = syn::parse::Parser::parse2(
        Punctuated::<Expr, Token![,]>::parse_terminated,
        mac.mac.tokens.clone(),
    )?;
    Ok(list.into_iter().collect())
}

struct FormatArgs {
    literal: String,
    arguments: Vec<(Option<String>, Expr)>,
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

enum JsonNode {
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
    fn span(&self) -> proc_macro2::Span {
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
