//! Typed intermediate representation for strict compilation.
//!
//! The IR keeps the source position of every node and records the effect of
//! every operation. Effects are explicit: `Pure` is an empty effect set, and a
//! host call, callback registration, allocation or native escape is carried
//! with the node that caused it. Lowering fails closed with a positioned
//! error when a construct cannot be represented exactly.
//!
//! The existing string emitter stays in use by `auto` and `off`. This module
//! is the typed front end for strict emission and the effect checker.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;

use proc_macro2::{Span as SourceSpan, TokenStream, TokenTree};
use syn::spanned::Spanned;

use crate::program::Program;
use crate::{Error, Options, strict};

/// Source position carried by an IR node.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Span {
    pub file: String,
    pub line: usize,
    pub column: usize,
}

impl Span {
    fn new(span: SourceSpan, file: &str) -> Self {
        let start = span.start();
        Self {
            file: file.to_owned(),
            line: start.line,
            column: start.column.saturating_add(1),
        }
    }

    /// Renders the position as `file:line:column`.
    #[must_use]
    pub fn render(&self) -> String {
        format!("{}:{}:{}", self.file, self.line, self.column)
    }
}

impl fmt::Display for Span {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.render())
    }
}

/// Exact integer width and signedness of a value.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct IntType {
    pub signed: bool,
    pub width: u8,
}

impl IntType {
    pub const I64: Self = Self {
        signed: true,
        width: 64,
    };

    /// Parses a Rust integer type name.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        let (signed, digits) = match name.strip_prefix('u') {
            Some(digits) => (false, digits),
            None => (true, name.strip_prefix('i')?),
        };
        let width = digits.parse::<u8>().ok()?;
        (matches!(width, 8 | 16 | 32 | 64 | 128))
            .then_some(Self { signed, width })
    }

    /// Checks that decimal digits fit this exact integer type.
    #[must_use]
    pub fn fits(self, digits: &str) -> bool {
        self.fits_value(digits, false)
    }

    fn fits_magnitude(self, digits: &str) -> bool {
        self.fits_value(digits, true)
    }

    fn fits_value(self, digits: &str, magnitude: bool) -> bool {
        let negative = digits.starts_with('-');
        if negative && !self.signed {
            return false;
        }
        let unsigned = digits.strip_prefix('-').unwrap_or(digits);
        let Ok(value) = unsigned.parse::<u128>() else {
            return false;
        };
        let bits = u32::from(self.width);
        if self.signed {
            let max = (1_u128 << bits.saturating_sub(1)).saturating_sub(1);
            let limit = if magnitude {
                max.saturating_add(1)
            } else {
                max
            };
            value <= limit
        } else {
            self.width == 128 || value < (1_u128 << bits)
        }
    }
}

/// Type of an IR value. Luau has one number type, so integer width and
/// signedness are kept explicitly instead of being inferred by the target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Ty {
    Unit,
    Bool,
    Int(IntType),
    Float,
    Str,
    Null,
    Value,
    Context,
    Node,
    Op,
    Optional(Box<Ty>),
    Array(Box<Ty>),
    Tuple(Vec<Ty>),
    Table(BTreeMap<String, Ty>),
    /// A value whose Rust type was not proven. Such a value can be passed
    /// through, but operations that need exact semantics must fail closed.
    Dynamic,
}

impl Ty {
    fn from_name(name: &str) -> Self {
        match name {
            "bool" => Self::Bool,
            "f32" | "f64" => Self::Float,
            "String" | "str" | "PathBuf" | "Path" | "char" => Self::Str,
            "Value" => Self::Value,
            "Context" => Self::Context,
            "Op" => Self::Op,
            "Node" => Self::Node,
            "Vec" | "BTreeMap" | "HashMap" | "Map" => Self::Dynamic,
            _ => IntType::parse(name).map_or(Self::Dynamic, Self::Int),
        }
    }

    fn of_type(ty: &syn::Type) -> Self {
        match ty {
            syn::Type::Reference(reference) => Self::of_type(&reference.elem),
            syn::Type::Paren(paren) => Self::of_type(&paren.elem),
            syn::Type::Group(group) => Self::of_type(&group.elem),
            syn::Type::Tuple(tuple) if tuple.elems.is_empty() => Self::Unit,
            syn::Type::Tuple(tuple) => {
                Self::Tuple(tuple.elems.iter().map(Self::of_type).collect())
            }
            syn::Type::Path(path) => {
                let Some(segment) = path.path.segments.last() else {
                    return Self::Dynamic;
                };
                match segment.ident.to_string().as_str() {
                    "Option" => Self::Optional(Box::new(Self::Dynamic)),
                    "Vec" => Self::Array(Box::new(Self::Dynamic)),
                    "Result" => Self::Dynamic,
                    name => Self::from_name(name),
                }
            }
            _ => Self::Dynamic,
        }
    }

    fn numeric(self, other: Self, span: &Span) -> Result<Self, Error> {
        match (self, other) {
            (Self::Int(left), Self::Int(right)) if left == right => {
                Ok(Self::Int(left))
            }
            (Self::Int(_), Self::Int(_)) => Err(error(
                span,
                "integer types with different width or signedness",
            )),
            (Self::Float, Self::Int(_) | Self::Float)
            | (Self::Int(_), Self::Float) => Ok(Self::Float),
            (Self::Dynamic, Self::Dynamic) => Ok(Self::Dynamic),
            (Self::Dynamic, value) | (value, Self::Dynamic)
                if matches!(value, Self::Int(_) | Self::Float) =>
            {
                Ok(Self::Dynamic)
            }
            _ => Err(error(span, "operator needs numeric operands")),
        }
    }
}

/// The API 57 return behavior of a direct host call.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum HostKind {
    Void,
    Bool,
    Value,
    Pair,
    Handler,
    Namespace,
    Node,
}

impl From<strict::MemberKind> for HostKind {
    fn from(kind: strict::MemberKind) -> Self {
        match kind {
            strict::MemberKind::Void => Self::Void,
            strict::MemberKind::Bool => Self::Bool,
            strict::MemberKind::Value => Self::Value,
            strict::MemberKind::Pair => Self::Pair,
            strict::MemberKind::Handler => Self::Handler,
            strict::MemberKind::Namespace => Self::Namespace,
            strict::MemberKind::Node => Self::Node,
        }
    }
}

impl HostKind {
    fn ty(self) -> Ty {
        match self {
            Self::Void => Ty::Unit,
            Self::Bool => Ty::Bool,
            Self::Pair => Ty::Tuple(vec![Ty::Value, Ty::Value]),
            Self::Node => Ty::Node,
            Self::Value => Ty::Value,
            Self::Handler | Self::Namespace => Ty::Dynamic,
        }
    }
}

/// What an allocation produces, for callers that specialize table shapes.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AllocKind {
    Vec,
    Table,
    Node,
    Op,
    Str,
}

/// One explicit effect. `Pure` is represented by an empty [`Effects`].
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Effect {
    Host { path: String, kind: HostKind },
    Callback { id: String },
    Allocate { kind: AllocKind },
    Native { reason: String },
}

/// Effects of one IR node or function, in a stable order.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Effects(BTreeSet<Effect>);

impl Effects {
    pub fn pure() -> Self {
        Self::default()
    }

    pub fn one(effect: Effect) -> Self {
        let mut effects = Self::default();
        effects.insert(effect);
        effects
    }

    pub fn insert(&mut self, effect: Effect) {
        self.0.insert(effect);
    }

    pub fn union(&self, other: &Self) -> Self {
        Self(self.0.union(&other.0).cloned().collect())
    }

    pub fn is_pure(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Effect> {
        self.0.iter()
    }
}

/// Unique identity of a Rust binding. A shadowing `let` gets a new id, even
/// when the source uses the same name.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BindingId(u64);

impl fmt::Display for BindingId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "b{}", self.0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Binding {
    pub id: BindingId,
    pub name: String,
    pub ty: Ty,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Literal {
    Str(String),
    Int { text: String, ty: IntType },
    Float(String),
    Bool(bool),
    Null,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnaryOp {
    Neg,
    Not,
    Deref,
    Ref,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CallTarget {
    Path(String),
    Method(String),
    Expr(Box<Expr>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExprKind {
    Literal(Literal),
    Binding(Binding),
    Path(String),
    Call {
        target: CallTarget,
        args: Vec<Expr>,
    },
    Field {
        base: Box<Expr>,
        name: String,
    },
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    Cast {
        value: Box<Expr>,
        target: Box<Ty>,
    },
    Array(Vec<Expr>),
    Repeat {
        value: Box<Expr>,
        length: Box<Expr>,
    },
    Tuple(Vec<Expr>),
    Table(Vec<(String, Expr)>),
    Macro {
        name: String,
        tokens: String,
    },
    If {
        branches: Vec<(Expr, Block)>,
        otherwise: Option<Block>,
    },
    Block(Block),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub ty: Ty,
    pub effects: Effects,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StmtKind {
    Let {
        binding: Binding,
        value: Expr,
    },
    Assign {
        target: Expr,
        value: Expr,
    },
    Expr(Expr),
    Return(Option<Expr>),
    If {
        branches: Vec<(Expr, Block)>,
        otherwise: Option<Block>,
    },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub effects: Effects,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub effects: Effects,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Function {
    pub name: String,
    pub params: Vec<Binding>,
    pub ret: Ty,
    pub body: Block,
    pub effects: Effects,
    pub span: Span,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectSite {
    pub effect: Effect,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IrProgram {
    pub module: String,
    pub entry: String,
    pub functions: Vec<Function>,
    pub effects: Effects,
    pub effect_sites: Vec<EffectSite>,
    pub span: Span,
}

/// Modes that decide which effects are permitted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Strict,
    Auto,
    Off,
}

fn error(span: &Span, message: impl Into<String>) -> Error {
    Error {
        message: message.into(),
        line: span.line,
        column: span.column,
        file: Some(span.file.clone()),
        fatal: false,
    }
}

/// Checks that the program's effects are valid for a compile mode.
///
/// Strict mode rejects native escapes at their source position. Auto may place
/// a function-level native boundary, and off does not run the IR at all.
pub fn validate(program: &IrProgram, mode: Mode) -> Result<(), Error> {
    if mode == Mode::Strict {
        for site in &program.effect_sites {
            if let Effect::Native { reason } = &site.effect {
                return Err(error(
                    &site.span,
                    format!("native effect in strict mode: {reason}"),
                ));
            }
        }
    }
    Ok(())
}

/// Lowers a handler and the local functions it calls into the typed IR.
pub fn lower(source: &str, options: &Options<'_>) -> Result<IrProgram, Error> {
    let program = Program::new(options.module, source, options.modules)?;
    let handler = program.handler(options.module, options.handler)?;
    let mut lowerer = Lowerer::new(&program, options.module);
    let entry = lowerer.function(&handler.module, handler.function)?;
    let mut effects = entry.effects.clone();
    for function in &lowerer.functions {
        effects = effects.union(&function.effects);
    }
    Ok(IrProgram {
        module: options.module.to_owned(),
        entry: entry.name.clone(),
        functions: lowerer.functions,
        effects,
        effect_sites: lowerer.sites,
        span: entry.span.clone(),
    })
}

/// Lowers a handler and checks its effects against strict mode.
pub fn lower_strict(
    source: &str,
    options: &Options<'_>,
) -> Result<IrProgram, Error> {
    let program = lower(source, options)?;
    validate(&program, Mode::Strict)?;
    Ok(program)
}

struct Lowerer<'a> {
    program: &'a Program,
    module: String,
    file: String,
    next_binding: u64,
    functions: Vec<Function>,
    sites: Vec<EffectSite>,
    stack: Vec<String>,
}

impl<'a> Lowerer<'a> {
    fn new(program: &'a Program, module: &str) -> Self {
        Self {
            program,
            module: module.to_owned(),
            file: module.to_owned(),
            next_binding: 0,
            functions: Vec::new(),
            sites: Vec::new(),
            stack: Vec::new(),
        }
    }

    fn span(&self, span: SourceSpan) -> Span {
        Span::new(span, &self.file)
    }

    fn binding(&mut self, name: String, ty: Ty, span: SourceSpan) -> Binding {
        self.next_binding = self.next_binding.saturating_add(1);
        Binding {
            id: BindingId(self.next_binding),
            name,
            ty,
            span: self.span(span),
        }
    }

    fn function(
        &mut self,
        module: &str,
        function: &syn::ItemFn,
    ) -> Result<Function, Error> {
        let name = function.sig.ident.to_string();
        let key = format!("{module}::{name}");
        let previous_file =
            std::mem::replace(&mut self.file, module.to_owned());
        if self.functions.iter().any(|found| found.name == key) {
            let found = self
                .functions
                .iter()
                .find(|found| found.name == key)
                .cloned()
                .ok_or_else(|| {
                    error(&self.span(function.span()), "missing function")
                });
            self.file = previous_file;
            return found;
        }
        if self.stack.contains(&key) {
            self.file = previous_file;
            return Err(error(
                &self.span(function.span()),
                format!(
                    "recursive function `{name}` has no proven Luau effect"
                ),
            ));
        }
        self.stack.push(key.clone());
        let mut scope = vec![HashMap::new()];
        let mut params = Vec::new();
        for (pattern, ty) in crate::program::typed_args(function) {
            self.pattern(pattern, &Ty::of_type(ty), &mut scope, &mut params)?;
        }
        let body = self.block(&function.block, &mut scope)?;
        self.stack.pop();
        let function_span = self.span(function.span());
        self.file = previous_file;
        let ret = match &function.sig.output {
            syn::ReturnType::Default => Ty::Unit,
            syn::ReturnType::Type(_, ty) => Ty::of_type(ty),
        };
        let effects = body.effects.clone();
        let result = Function {
            name: key,
            params,
            ret,
            body,
            effects,
            span: function_span,
        };
        self.functions.push(result.clone());
        Ok(result)
    }

    fn pattern(
        &mut self,
        pattern: &syn::Pat,
        ty: &Ty,
        scope: &mut [HashMap<String, Binding>],
        out: &mut Vec<Binding>,
    ) -> Result<(), Error> {
        match pattern {
            syn::Pat::Ident(ident) => {
                let binding = self.binding(
                    ident.ident.to_string(),
                    ty.clone(),
                    ident.span(),
                );
                if let Some(slot) = scope.last_mut() {
                    slot.insert(binding.name.clone(), binding.clone());
                }
                out.push(binding);
                Ok(())
            }
            syn::Pat::Wild(wild) => {
                let binding =
                    self.binding("_".to_owned(), ty.clone(), wild.span());
                out.push(binding);
                Ok(())
            }
            syn::Pat::Type(annotated) => self.pattern(
                &annotated.pat,
                &Ty::of_type(&annotated.ty),
                scope,
                out,
            ),
            syn::Pat::Tuple(tuple) => {
                for (index, element) in tuple.elems.iter().enumerate() {
                    let element_ty = match ty {
                        Ty::Tuple(parts) => {
                            parts.get(index).cloned().unwrap_or(Ty::Dynamic)
                        }
                        _ => Ty::Dynamic,
                    };
                    self.pattern(element, &element_ty, scope, out)?;
                }
                Ok(())
            }
            syn::Pat::Struct(pattern) => {
                for element in &pattern.fields {
                    self.pattern(&element.pat, &Ty::Dynamic, scope, out)?;
                }
                Ok(())
            }
            _ => Err(error(
                &self.span(pattern.span()),
                "pattern is not proven by the typed IR",
            )),
        }
    }

    fn block(
        &mut self,
        block: &syn::Block,
        scope: &mut Vec<HashMap<String, Binding>>,
    ) -> Result<Block, Error> {
        scope.push(HashMap::new());
        let mut stmts = Vec::new();
        let mut effects = Effects::pure();
        for statement in &block.stmts {
            for lowered in self.statement(statement, scope)? {
                effects = effects.union(&lowered.effects);
                stmts.push(lowered);
            }
        }
        scope.pop();
        Ok(Block {
            stmts,
            effects,
            span: self.span(block.span()),
        })
    }

    fn statement(
        &mut self,
        statement: &syn::Stmt,
        scope: &mut Vec<HashMap<String, Binding>>,
    ) -> Result<Vec<Stmt>, Error> {
        match statement {
            syn::Stmt::Local(local) => {
                let init = match local.init.as_ref() {
                    Some(init) => self.expr(&init.expr, scope)?,
                    None => {
                        return Err(error(
                            &self.span(local.span()),
                            "an uninitialized binding has no proven Luau value",
                        ));
                    }
                };
                let mut bindings = Vec::new();
                self.pattern(&local.pat, &init.ty, scope, &mut bindings)?;
                let span = self.span(local.span());
                let effects = init.effects.clone();
                Ok(bindings
                    .into_iter()
                    .map(|binding| Stmt {
                        kind: StmtKind::Let {
                            binding,
                            value: pure(init.clone()),
                        },
                        effects: effects.clone(),
                        span: span.clone(),
                    })
                    .collect())
            }
            syn::Stmt::Expr(expression, _) => {
                let value = self.expr(expression, scope)?;
                let span = value.span.clone();
                let effects = value.effects.clone();
                Ok(vec![Stmt {
                    kind: StmtKind::Expr(pure(value)),
                    effects,
                    span,
                }])
            }
            syn::Stmt::Item(item) => match item {
                syn::Item::Use(_) => Ok(Vec::new()),
                _ => Err(error(
                    &self.span(item.span()),
                    "item statement is not proven by the typed IR",
                )),
            },
            syn::Stmt::Macro(mac) => {
                let expression = syn::Expr::Macro(syn::ExprMacro {
                    attrs: Vec::new(),
                    mac: mac.mac.clone(),
                });
                let value = self.expr(&expression, scope)?;
                let span = value.span.clone();
                let effects = value.effects.clone();
                Ok(vec![Stmt {
                    kind: StmtKind::Expr(pure(value)),
                    effects,
                    span,
                }])
            }
        }
    }
    fn lookup(
        &self,
        name: &str,
        scope: &[HashMap<String, Binding>],
    ) -> Option<Binding> {
        scope
            .iter()
            .rev()
            .find_map(|level| level.get(name).cloned())
    }

    fn expr(
        &mut self,
        expression: &syn::Expr,
        scope: &mut Vec<HashMap<String, Binding>>,
    ) -> Result<Expr, Error> {
        let value = self.expr_inner(expression, scope)?;
        for effect in value.effects.iter() {
            self.sites.push(EffectSite {
                effect: effect.clone(),
                span: value.span.clone(),
            });
        }
        Ok(value)
    }

    fn expr_inner(
        &mut self,
        expression: &syn::Expr,
        scope: &mut Vec<HashMap<String, Binding>>,
    ) -> Result<Expr, Error> {
        let span = self.span(expression.span());
        match expression {
            syn::Expr::Lit(literal) => self.literal(literal, &span),
            syn::Expr::Path(path) => {
                let segments = path_segments(&path.path);
                let Some(segments) = segments else {
                    return Err(error(
                        &span,
                        "path is not proven by the typed IR",
                    ));
                };
                if segments.len() == 1
                    && let Some(binding) = self.lookup(&segments[0], scope)
                {
                    return Ok(Expr {
                        kind: ExprKind::Binding(binding.clone()),
                        ty: binding.ty,
                        effects: Effects::pure(),
                        span,
                    });
                }
                let name = segments.join("::");
                Ok(Expr {
                    kind: ExprKind::Path(name.clone()),
                    ty: Ty::from_name(
                        segments.last().map_or("", String::as_str),
                    ),
                    effects: Effects::pure(),
                    span,
                })
            }
            syn::Expr::Binary(binary) => {
                let left = self.expr(&binary.left, scope)?;
                let right = self.expr(&binary.right, scope)?;
                let op = binary_op(&binary.op).ok_or_else(|| {
                    error(&span, "operator is not proven by the typed IR")
                })?;
                let ty = match op {
                    BinaryOp::Eq
                    | BinaryOp::Ne
                    | BinaryOp::Lt
                    | BinaryOp::Le
                    | BinaryOp::Gt
                    | BinaryOp::Ge => Ty::Bool,
                    BinaryOp::And | BinaryOp::Or => Ty::Bool,
                    _ => left.ty.clone().numeric(right.ty.clone(), &span)?,
                };
                let effects = left.effects.union(&right.effects);
                Ok(Expr {
                    kind: ExprKind::Binary {
                        op,
                        left: Box::new(left),
                        right: Box::new(right),
                    },
                    ty,
                    effects,
                    span,
                })
            }
            syn::Expr::Unary(unary) => {
                let op = match unary.op {
                    syn::UnOp::Neg(_) => UnaryOp::Neg,
                    syn::UnOp::Not(_) => UnaryOp::Not,
                    syn::UnOp::Deref(_) => UnaryOp::Deref,
                    _ => {
                        return Err(error(
                            &span,
                            "unary operator is not proven by the typed IR",
                        ));
                    }
                };
                let operand = if op == UnaryOp::Neg
                    && let syn::Expr::Lit(literal) = unary.expr.as_ref()
                    && let syn::Lit::Int(value) = &literal.lit
                {
                    let kind =
                        IntType::parse(value.suffix()).unwrap_or(IntType::I64);
                    let digits = value.base10_digits();
                    if !kind.fits(digits)
                        && kind.signed
                        && kind.fits_magnitude(digits)
                    {
                        Expr {
                            kind: ExprKind::Literal(Literal::Int {
                                text: digits.to_owned(),
                                ty: kind,
                            }),
                            ty: Ty::Int(kind),
                            effects: Effects::pure(),
                            span: self.span(literal.span()),
                        }
                    } else {
                        self.expr(&unary.expr, scope)?
                    }
                } else {
                    self.expr(&unary.expr, scope)?
                };
                let ty = match op {
                    UnaryOp::Neg => match operand.ty {
                        Ty::Int(width) if width.signed => Ty::Int(width),
                        Ty::Int(_) => {
                            return Err(error(
                                &span,
                                "negating an unsigned integer has no exact Luau value",
                            ));
                        }
                        Ty::Float | Ty::Dynamic => Ty::Dynamic,
                        _ => {
                            return Err(error(
                                &span,
                                "negation needs a number",
                            ));
                        }
                    },
                    UnaryOp::Not => Ty::Bool,
                    UnaryOp::Deref => operand.ty.clone(),
                    UnaryOp::Ref => Ty::Dynamic,
                };
                let effects = operand.effects.clone();
                Ok(Expr {
                    kind: ExprKind::Unary {
                        op,
                        operand: Box::new(operand),
                    },
                    ty,
                    effects,
                    span,
                })
            }
            syn::Expr::Reference(reference) => {
                let operand = self.expr(&reference.expr, scope)?;
                Ok(Expr {
                    kind: ExprKind::Unary {
                        op: UnaryOp::Ref,
                        operand: Box::new(operand.clone()),
                    },
                    ty: Ty::Optional(Box::new(operand.ty)),
                    effects: operand.effects,
                    span,
                })
            }
            syn::Expr::Paren(paren) => self.expr(&paren.expr, scope),
            syn::Expr::Group(group) => self.expr(&group.expr, scope),
            syn::Expr::Cast(cast) => {
                let operand = self.expr(&cast.expr, scope)?;
                let target = Ty::of_type(&cast.ty);
                Ok(Expr {
                    kind: ExprKind::Cast {
                        value: Box::new(pure(operand.clone())),
                        target: Box::new(target.clone()),
                    },
                    ty: target,
                    effects: operand.effects,
                    span,
                })
            }
            syn::Expr::Field(field) => {
                let base = self.expr(&field.base, scope)?;
                let name = match &field.member {
                    syn::Member::Named(name) => name.to_string(),
                    syn::Member::Unnamed(index) => index.index.to_string(),
                };
                let effects = base.effects.clone();
                Ok(Expr {
                    kind: ExprKind::Field {
                        base: Box::new(base),
                        name,
                    },
                    ty: Ty::Dynamic,
                    effects,
                    span,
                })
            }
            syn::Expr::Index(index) => {
                let base = self.expr(&index.expr, scope)?;
                let key = self.expr(&index.index, scope)?;
                let effects = base.effects.union(&key.effects);
                Ok(Expr {
                    kind: ExprKind::Index {
                        base: Box::new(base),
                        index: Box::new(key),
                    },
                    ty: Ty::Dynamic,
                    effects,
                    span,
                })
            }
            syn::Expr::Array(array) => {
                let mut elements = Vec::new();
                let mut effects = Effects::pure();
                for element in &array.elems {
                    let value = self.expr(element, scope)?;
                    if value.ty == Ty::Null {
                        return Err(error(
                            &value.span,
                            "JSON null in an array slot has no exact Luau value",
                        ));
                    }
                    effects = effects.union(&value.effects);
                    elements.push(pure(value));
                }
                let ty = elements
                    .first()
                    .map_or(Ty::Array(Box::new(Ty::Dynamic)), |value| {
                        Ty::Array(Box::new(value.ty.clone()))
                    });
                Ok(Expr {
                    kind: ExprKind::Array(elements),
                    ty,
                    effects,
                    span,
                })
            }
            syn::Expr::Repeat(repeat) => {
                let value = self.expr(&repeat.expr, scope)?;
                let length = self.expr(&repeat.len, scope)?;
                let effects = value.effects.union(&length.effects);
                Ok(Expr {
                    kind: ExprKind::Repeat {
                        value: Box::new(pure(value.clone())),
                        length: Box::new(pure(length)),
                    },
                    ty: Ty::Array(Box::new(value.ty)),
                    effects,
                    span,
                })
            }
            syn::Expr::Tuple(tuple) => {
                let mut values = Vec::new();
                let mut effects = Effects::pure();
                for element in &tuple.elems {
                    let value = self.expr(element, scope)?;
                    if is_null(&value) {
                        return Err(error(
                            &value.span,
                            "JSON null in a tuple slot has no exact Luau value",
                        ));
                    }
                    effects = effects.union(&value.effects);
                    values.push(pure(value));
                }
                let ty = Ty::Tuple(
                    values.iter().map(|value| value.ty.clone()).collect(),
                );
                Ok(Expr {
                    kind: ExprKind::Tuple(values),
                    ty,
                    effects,
                    span,
                })
            }
            syn::Expr::Struct(structure) => {
                let mut fields = Vec::new();
                let mut effects = Effects::one(Effect::Allocate {
                    kind: AllocKind::Table,
                });
                for field in &structure.fields {
                    let value = self.expr(&field.expr, scope)?;
                    if value.ty == Ty::Null {
                        return Err(error(
                            &value.span,
                            "JSON null in a table slot has no exact Luau value",
                        ));
                    }
                    effects = effects.union(&value.effects);
                    fields.push((member_name(&field.member), pure(value)));
                }
                Ok(Expr {
                    kind: ExprKind::Table(fields),
                    ty: Ty::from_name(
                        &structure
                            .path
                            .segments
                            .last()
                            .map_or_else(String::new, |segment| {
                                segment.ident.to_string()
                            }),
                    ),
                    effects,
                    span,
                })
            }
            syn::Expr::Macro(mac) => self.macro_expr(mac, &span),
            syn::Expr::Call(call) => self.call(call, scope, &span),
            syn::Expr::MethodCall(call) => self.method_call(call, scope, &span),
            syn::Expr::If(conditional) => {
                let mut branches = Vec::new();
                let mut effects = Effects::pure();
                let condition = self.expr(&conditional.cond, scope)?;
                effects = effects.union(&condition.effects);
                let then = self.block(&conditional.then_branch, scope)?;
                effects = effects.union(&then.effects);
                branches.push((pure(condition.clone()), then.clone()));
                let otherwise = match conditional.else_branch.as_ref() {
                    Some((_, expression)) => {
                        let lowered = self.expr(expression, scope)?;
                        effects = effects.union(&lowered.effects);
                        match lowered {
                            Expr {
                                kind:
                                    ExprKind::If {
                                        branches: nested,
                                        otherwise,
                                    },
                                ..
                            } => {
                                for (condition, body) in nested {
                                    branches.push((condition, body));
                                }
                                otherwise
                            }
                            Expr {
                                kind: ExprKind::Block(block),
                                ..
                            } => Some(block),
                            value => Some(Block {
                                stmts: vec![Stmt {
                                    kind: StmtKind::Return(Some(pure(value))),
                                    effects: Effects::pure(),
                                    span: span.clone(),
                                }],
                                effects: Effects::pure(),
                                span: span.clone(),
                            }),
                        }
                    }
                    None => None,
                };
                Ok(Expr {
                    kind: ExprKind::If {
                        branches,
                        otherwise,
                    },
                    ty: Ty::Dynamic,
                    effects,
                    span,
                })
            }
            syn::Expr::Block(block) => {
                let lowered = self.block(&block.block, scope)?;
                let effects = lowered.effects.clone();
                Ok(Expr {
                    kind: ExprKind::Block(lowered),
                    ty: Ty::Dynamic,
                    effects,
                    span,
                })
            }
            syn::Expr::Return(return_expr) => {
                let value = return_expr
                    .expr
                    .as_ref()
                    .map(|expression| self.expr(expression, scope))
                    .transpose()?;
                let effects = value
                    .as_ref()
                    .map_or_else(Effects::pure, |value| value.effects.clone());
                Ok(Expr {
                    kind: ExprKind::Block(Block {
                        stmts: vec![Stmt {
                            kind: StmtKind::Return(value.map(pure)),
                            effects: Effects::pure(),
                            span: span.clone(),
                        }],
                        effects: Effects::pure(),
                        span: span.clone(),
                    }),
                    ty: Ty::Unit,
                    effects,
                    span,
                })
            }
            syn::Expr::Try(_) => Err(error(
                &span,
                "`?` needs an early return path the typed IR cannot prove",
            )),
            syn::Expr::Await(await_expr) => {
                let value = self.expr(&await_expr.base, scope)?;
                let mut effects = value.effects.clone();
                effects.insert(Effect::Native {
                    reason: "`await` needs a native async runtime".to_owned(),
                });
                Ok(Expr {
                    kind: ExprKind::Unary {
                        op: UnaryOp::Deref,
                        operand: Box::new(value),
                    },
                    ty: Ty::Dynamic,
                    effects,
                    span,
                })
            }
            syn::Expr::Unsafe(unsafe_expr) => {
                let lowered = self.block(&unsafe_expr.block, scope)?;
                let mut effects = lowered.effects.clone();
                effects.insert(Effect::Native {
                    reason: "unsafe block has no Luau semantics".to_owned(),
                });
                Ok(Expr {
                    kind: ExprKind::Block(lowered),
                    ty: Ty::Dynamic,
                    effects,
                    span,
                })
            }
            syn::Expr::Closure(closure) => {
                scope.push(HashMap::new());
                let mut params = Vec::new();
                for input in &closure.inputs {
                    self.pattern(input, &Ty::Dynamic, scope, &mut params)?;
                }
                let body = match closure.body.as_ref() {
                    syn::Expr::Block(block) => {
                        self.block(&block.block, scope)?
                    }
                    expression => {
                        let value = self.expr(expression, scope)?;
                        let effects = value.effects.clone();
                        let value_span = value.span.clone();
                        Block {
                            stmts: vec![Stmt {
                                kind: StmtKind::Return(Some(pure(value))),
                                effects: Effects::pure(),
                                span: value_span.clone(),
                            }],
                            effects,
                            span: value_span,
                        }
                    }
                };
                scope.pop();
                let effects = body.effects.clone();
                Ok(Expr {
                    kind: ExprKind::Block(body),
                    ty: Ty::Dynamic,
                    effects,
                    span,
                })
            }
            syn::Expr::Range(range) => {
                let start =
                    range.start.as_ref().map(|value| self.expr(value, scope));
                let start = start.transpose()?;
                let end =
                    range.end.as_ref().map(|value| self.expr(value, scope));
                let end = end.transpose()?;
                let mut effects = Effects::pure();
                if let Some(value) = &start {
                    effects = effects.union(&value.effects);
                }
                if let Some(value) = &end {
                    effects = effects.union(&value.effects);
                }
                Ok(Expr {
                    kind: ExprKind::Tuple(
                        start.into_iter().chain(end).map(pure).collect(),
                    ),
                    ty: Ty::Dynamic,
                    effects,
                    span,
                })
            }
            syn::Expr::Assign(assign) => {
                let target = self.expr(&assign.left, scope)?;
                let value = self.expr(&assign.right, scope)?;
                if is_null(&value) {
                    return Err(error(
                        &value.span,
                        "JSON null assignment has no exact Luau value",
                    ));
                }
                let effects = target.effects.union(&value.effects);
                Ok(Expr {
                    kind: ExprKind::Block(Block {
                        stmts: vec![Stmt {
                            kind: StmtKind::Assign {
                                target: pure(target),
                                value: pure(value),
                            },
                            effects: Effects::pure(),
                            span: span.clone(),
                        }],
                        effects: Effects::pure(),
                        span: span.clone(),
                    }),
                    ty: Ty::Unit,
                    effects,
                    span,
                })
            }
            _ => Err(error(&span, "expression is not proven by the typed IR")),
        }
    }

    fn literal(
        &self,
        literal: &syn::ExprLit,
        span: &Span,
    ) -> Result<Expr, Error> {
        let (value, ty) = match &literal.lit {
            syn::Lit::Str(value) => (Literal::Str(value.value()), Ty::Str),
            syn::Lit::Char(value) => {
                (Literal::Str(value.value().to_string()), Ty::Str)
            }
            syn::Lit::Byte(value) => (
                Literal::Int {
                    text: value.value().to_string(),
                    ty: IntType {
                        signed: false,
                        width: 8,
                    },
                },
                Ty::Int(IntType {
                    signed: false,
                    width: 8,
                }),
            ),
            syn::Lit::Int(value) => {
                let ty = IntType::parse(value.suffix()).unwrap_or(IntType::I64);
                if !ty.fits(value.base10_digits()) {
                    return Err(error(
                        span,
                        "integer literal does not fit its type",
                    ));
                }
                (
                    Literal::Int {
                        text: value.base10_digits().to_owned(),
                        ty,
                    },
                    Ty::Int(ty),
                )
            }
            syn::Lit::Float(value) => {
                (Literal::Float(value.base10_digits().to_owned()), Ty::Float)
            }
            syn::Lit::Bool(value) => (Literal::Bool(value.value), Ty::Bool),
            syn::Lit::ByteStr(_) => {
                return Err(error(
                    span,
                    "byte string literal has no exact Luau string value",
                ));
            }
            _ => {
                return Err(error(
                    span,
                    "literal is not proven by the typed IR",
                ));
            }
        };
        Ok(Expr {
            kind: ExprKind::Literal(value),
            ty,
            effects: Effects::pure(),
            span: span.clone(),
        })
    }

    fn macro_expr(
        &mut self,
        mac: &syn::ExprMacro,
        span: &Span,
    ) -> Result<Expr, Error> {
        let name = mac
            .mac
            .path
            .segments
            .last()
            .map_or_else(String::new, |segment| segment.ident.to_string())
            .to_owned();
        let tokens = mac.mac.tokens.to_string();
        if name == "json" && tokens.trim() == "null" {
            return Ok(Expr {
                kind: ExprKind::Literal(Literal::Null),
                ty: Ty::Null,
                effects: Effects::pure(),
                span: span.clone(),
            });
        }
        if name == "json" && contains_null(mac.mac.tokens.clone()) {
            return Err(error(
                span,
                "JSON null in a macro has no exact Luau value",
            ));
        }
        let kind = match name.as_str() {
            "json" | "vec" | "json_node" => AllocKind::Table,
            "format" => AllocKind::Str,
            "panic" | "todo" | "unimplemented" => {
                return Ok(Expr {
                    kind: ExprKind::Macro {
                        name: name.clone(),
                        tokens,
                    },
                    ty: Ty::Unit,
                    effects: Effects::one(Effect::Native {
                        reason: format!("panic macro `{name}`"),
                    }),
                    span: span.clone(),
                });
            }
            _ => {
                return Err(error(
                    span,
                    format!("macro `{name}` has no proven Luau semantics"),
                ));
            }
        };
        let ty = match name.as_str() {
            "vec" => Ty::Array(Box::new(Ty::Dynamic)),
            "format" | "json_node" => Ty::Str,
            _ => Ty::Value,
        };
        Ok(Expr {
            kind: ExprKind::Macro { name, tokens },
            ty,
            effects: Effects::one(Effect::Allocate { kind }),
            span: span.clone(),
        })
    }

    fn call(
        &mut self,
        call: &syn::ExprCall,
        scope: &mut Vec<HashMap<String, Binding>>,
        span: &Span,
    ) -> Result<Expr, Error> {
        let (target, receiver_ty) = match call.func.as_ref() {
            syn::Expr::Path(path) => {
                let segments = path_segments(&path.path).ok_or_else(|| {
                    error(span, "call path is not proven by the typed IR")
                })?;
                if segments.len() == 1
                    && let Some(binding) = self.lookup(&segments[0], scope)
                {
                    let value = Expr {
                        kind: ExprKind::Binding(binding.clone()),
                        ty: binding.ty,
                        effects: Effects::pure(),
                        span: self.span(path.span()),
                    };
                    let ty = value.ty.clone();
                    (CallTarget::Expr(Box::new(value)), Some(ty))
                } else {
                    (CallTarget::Path(segments.join("::")), Some(Ty::Dynamic))
                }
            }
            syn::Expr::Field(field) => {
                let base = self.expr(&field.base, scope)?;
                let name = match &field.member {
                    syn::Member::Named(name) => name.to_string(),
                    syn::Member::Unnamed(index) => index.index.to_string(),
                };
                let ty = base.ty.clone();
                (
                    CallTarget::Method(format!(
                        "{}.{}",
                        base_text(&base),
                        name
                    )),
                    Some(ty),
                )
            }
            expression => {
                let value = self.expr(expression, scope)?;
                (CallTarget::Expr(Box::new(value)), Some(Ty::Dynamic))
            }
        };
        let mut args = Vec::new();
        let mut effects = match &target {
            CallTarget::Path(path) => self.classify_path(path, span)?,
            CallTarget::Method(path) => {
                self.classify_method(path, receiver_ty.clone(), span)?
            }
            CallTarget::Expr(value) => value.effects.clone(),
        };
        for argument in &call.args {
            let value = self.expr(argument, scope)?;
            effects = effects.union(&value.effects);
            args.push(value);
        }
        let ty = self.call_type(&target, &args, span)?;
        Ok(Expr {
            kind: ExprKind::Call { target, args },
            ty,
            effects,
            span: span.clone(),
        })
    }

    fn method_call(
        &mut self,
        call: &syn::ExprMethodCall,
        scope: &mut Vec<HashMap<String, Binding>>,
        span: &Span,
    ) -> Result<Expr, Error> {
        let mut values = Vec::new();
        let receiver = self.expr(&call.receiver, scope)?;
        let receiver_ty = receiver.ty.clone();
        let mut effects = receiver.effects.clone();
        values.push(receiver);
        for argument in &call.args {
            let value = self.expr(argument, scope)?;
            effects = effects.union(&value.effects);
            values.push(value);
        }
        let name = call.method.to_string();
        effects = effects.union(&self.classify_method(
            &name,
            Some(receiver_ty),
            span,
        )?);
        let ty = self.method_type(&name)?;
        Ok(Expr {
            kind: ExprKind::Call {
                target: CallTarget::Method(name),
                args: values,
            },
            ty,
            effects,
            span: span.clone(),
        })
    }

    fn classify_path(
        &mut self,
        path: &str,
        span: &Span,
    ) -> Result<Effects, Error> {
        let segments = path.split("::").map(str::to_owned).collect::<Vec<_>>();
        if let Some((root, rest)) = segments.split_first()
            && let Some(kind) = strict::member(root, rest)
        {
            return Ok(Effects::one(Effect::Host {
                path: strict::member_path(root, rest),
                kind: kind.into(),
            }));
        }
        if path == "ui::callback" || path == "ui::call" {
            return Ok(Effects::one(Effect::Callback {
                id: path.to_owned(),
            }));
        }
        if segments.first().is_some_and(|root| {
            strict::groups().any(|(known, _)| known == root.as_str())
        }) {
            return Err(error(
                span,
                format!(
                    "unknown API member `{path}` has no proven Luau semantics"
                ),
            ));
        }
        if pure_path(path) {
            return Ok(Effects::pure());
        }
        if let Some((member, kind)) = host_path(path) {
            return Ok(Effects::one(Effect::Host {
                path: member.to_owned(),
                kind: kind.into(),
            }));
        }
        if native_path(path) {
            return Ok(Effects::one(Effect::Native {
                reason: format!("call to `{path}` needs native code"),
            }));
        }
        if allocation_path(path) {
            return Ok(Effects::one(Effect::Allocate {
                kind: if path.starts_with("Op::") {
                    AllocKind::Op
                } else if path.starts_with("ui::") {
                    AllocKind::Node
                } else if path.starts_with("String") {
                    AllocKind::Str
                } else {
                    AllocKind::Vec
                },
            }));
        }
        if let Some(resolved) = self.program.resolve(&self.module, &segments) {
            let item = resolved.function.clone();
            return Ok(self.function(&resolved.module, &item)?.effects);
        }
        Err(error(
            span,
            format!("call path `{path}` has no proven Luau semantics"),
        ))
    }

    fn classify_method(
        &mut self,
        name: &str,
        receiver: Option<Ty>,
        span: &Span,
    ) -> Result<Effects, Error> {
        let short = name.rsplit('.').next().unwrap_or(name);
        if receiver == Some(Ty::Context) {
            if let Some((path, kind)) = context_member(short) {
                return Ok(Effects::one(Effect::Host {
                    path: path.to_owned(),
                    kind,
                }));
            }
            return Err(error(
                span,
                format!("context method `{short}` has no proven API 57 member"),
            ));
        }
        match short {
            "callback" | "call" => Ok(Effects::one(Effect::Callback {
                id: short.to_owned(),
            })),
            _ if allocating_method(short) => {
                Ok(Effects::one(Effect::Allocate {
                    kind: AllocKind::Vec,
                }))
            }
            _ if pure_method(short) => Ok(Effects::pure()),
            _ => Err(error(
                span,
                format!("method `{short}` has no proven Luau semantics"),
            )),
        }
    }

    fn call_type(
        &mut self,
        target: &CallTarget,
        args: &[Expr],
        span: &Span,
    ) -> Result<Ty, Error> {
        if args.iter().any(|arg| {
            arg.ty == Ty::Null
                || matches!(&arg.kind, ExprKind::Literal(Literal::Null))
        }) {
            return Err(error(
                span,
                "JSON null argument has no exact Luau value",
            ));
        }
        match target {
            CallTarget::Path(path) => {
                let segments =
                    path.split("::").map(str::to_owned).collect::<Vec<_>>();
                if let Some((root, rest)) = segments.split_first()
                    && let Some(kind) = strict::member(root, rest)
                {
                    return Ok(HostKind::from(kind).ty());
                }
                if let Some((_, kind)) = host_path(path) {
                    return Ok(HostKind::from(kind).ty());
                }
                let name = segments.last().map_or("", String::as_str);
                if path == "ui::callback" || path == "ui::call" {
                    return Ok(Ty::Str);
                }
                if pure_path(path) {
                    return Ok(Ty::Dynamic);
                }
                if native_path(path) {
                    return Ok(Ty::Dynamic);
                }
                if name == "new" {
                    return Ok(Ty::from_name(
                        segments
                            .get(segments.len().saturating_sub(2))
                            .map_or("", String::as_str),
                    ));
                }
                if path.starts_with("Op::") {
                    return Ok(Ty::Op);
                }
                if path.starts_with("ui::") {
                    return Ok(Ty::Node);
                }
                if path == "String::from" || path == "format" {
                    return Ok(Ty::Str);
                }
                if let Some(resolved) =
                    self.program.resolve(&self.module, &segments)
                {
                    let item = resolved.function.clone();
                    return Ok(self.function(&resolved.module, &item)?.ret);
                }
                Err(error(
                    span,
                    format!("call path `{path}` has no proven Luau semantics"),
                ))
            }
            CallTarget::Method(name) => self.method_type(name),
            CallTarget::Expr(value) => Ok(value.ty.clone()),
        }
    }

    fn method_type(&self, name: &str) -> Result<Ty, Error> {
        let short = name.rsplit('.').next().unwrap_or(name);
        if let Some((_, kind)) = context_member(short) {
            return Ok(kind.ty());
        }
        if allocating_method(short) {
            return Ok(Ty::Dynamic);
        }
        if pure_method(short) {
            if short == "to_string" || short == "join" || short == "trim" {
                return Ok(Ty::Str);
            }
            if short == "len" || short == "count" {
                return Ok(Ty::Int(IntType::I64));
            }
            if short == "is_empty"
                || short == "contains"
                || short == "starts_with"
                || short == "ends_with"
            {
                return Ok(Ty::Bool);
            }
            if short == "callback" || short == "call" {
                return Ok(Ty::Str);
            }
            return Ok(Ty::Dynamic);
        }
        Ok(Ty::Dynamic)
    }
}

/// Clears the node's own effects when it becomes a child of another node.
/// The child statement or expression still carries its local effects.
fn pure(mut expr: Expr) -> Expr {
    expr.effects = Effects::pure();
    expr
}

/// Whether a value is JSON null, which Luau containers and calls cannot hold.
fn is_null(value: &Expr) -> bool {
    value.ty == Ty::Null
        || matches!(&value.kind, ExprKind::Literal(Literal::Null))
}

fn contains_null(tokens: TokenStream) -> bool {
    for token in tokens {
        match token {
            TokenTree::Ident(ident) if ident == "null" => return true,
            TokenTree::Group(group) if contains_null(group.stream()) => {
                return true;
            }
            _ => {}
        }
    }
    false
}

fn member_name(member: &syn::Member) -> String {
    match member {
        syn::Member::Named(name) => name.to_string(),
        syn::Member::Unnamed(index) => index.index.to_string(),
    }
}

fn path_segments(path: &syn::Path) -> Option<Vec<String>> {
    path.segments
        .iter()
        .map(|segment| {
            if matches!(segment.arguments, syn::PathArguments::None) {
                Some(segment.ident.to_string())
            } else {
                None
            }
        })
        .collect()
}

fn base_text(expr: &Expr) -> String {
    match &expr.kind {
        ExprKind::Path(path) => path.clone(),
        ExprKind::Binding(binding) => binding.name.clone(),
        _ => "$value".to_owned(),
    }
}

fn binary_op(op: &syn::BinOp) -> Option<BinaryOp> {
    Some(match op {
        syn::BinOp::Add(_) => BinaryOp::Add,
        syn::BinOp::Sub(_) => BinaryOp::Sub,
        syn::BinOp::Mul(_) => BinaryOp::Mul,
        syn::BinOp::Div(_) => BinaryOp::Div,
        syn::BinOp::Rem(_) => BinaryOp::Rem,
        syn::BinOp::BitAnd(_) => BinaryOp::BitAnd,
        syn::BinOp::BitOr(_) => BinaryOp::BitOr,
        syn::BinOp::BitXor(_) => BinaryOp::BitXor,
        syn::BinOp::Shl(_) => BinaryOp::Shl,
        syn::BinOp::Shr(_) => BinaryOp::Shr,
        syn::BinOp::Eq(_) => BinaryOp::Eq,
        syn::BinOp::Ne(_) => BinaryOp::Ne,
        syn::BinOp::Lt(_) => BinaryOp::Lt,
        syn::BinOp::Le(_) => BinaryOp::Le,
        syn::BinOp::Gt(_) => BinaryOp::Gt,
        syn::BinOp::Ge(_) => BinaryOp::Ge,
        syn::BinOp::And(_) => BinaryOp::And,
        syn::BinOp::Or(_) => BinaryOp::Or,
        _ => return None,
    })
}

fn allocation_path(path: &str) -> bool {
    path.starts_with("Op::")
        || path.starts_with("ui::")
        || path.starts_with("String::")
        || path == "Vec::new"
        || path == "format"
}

fn pure_path(path: &str) -> bool {
    matches!(path, "Ok" | "Some" | "None")
}

fn allocating_method(name: &str) -> bool {
    matches!(
        name,
        "push"
            | "insert"
            | "clone"
            | "to_owned"
            | "to_string"
            | "join"
            | "trim"
            | "replace"
            | "split"
            | "as_str"
            | "to_string_lossy"
            | "into_iter"
            | "iter"
    )
}

fn pure_method(name: &str) -> bool {
    matches!(
        name,
        "len"
            | "count"
            | "is_empty"
            | "contains"
            | "starts_with"
            | "ends_with"
            | "parse"
            | "unwrap_or"
            | "unwrap_or_else"
            | "get"
            | "first"
            | "last"
            | "callback"
            | "call"
    )
}

fn host_path(path: &str) -> Option<(&'static str, strict::MemberKind)> {
    match path {
        "std::env::var" | "env::var" => {
            Some(("noctalia.getenv", strict::MemberKind::Value))
        }
        "std::time::SystemTime::now" | "SystemTime::now" => {
            Some(("noctalia.nowMs", strict::MemberKind::Handler))
        }
        _ => None,
    }
}

fn native_path(path: &str) -> bool {
    path.starts_with("std::fs::")
        || path.starts_with("std::net::")
        || path.starts_with("std::process::")
        || path.starts_with("std::thread::")
        || path.starts_with("std::env::")
        || path.starts_with("libc::")
        || path.starts_with("Command::")
}

fn context_member(name: &str) -> Option<(&'static str, HostKind)> {
    Some(match name {
        "config_str" | "config_bool" => {
            ("noctalia.getConfig", HostKind::Handler)
        }
        "setting" | "setting_value" => {
            ("noctalia.getSetting", HostKind::Handler)
        }
        "file_exists" => ("noctalia.fileExists", HostKind::Value),
        "read_file" | "read_file_bytes" => {
            ("noctalia.readFile", HostKind::Value)
        }
        "write_file" | "write_file_bytes" => {
            ("noctalia.writeFile", HostKind::Pair)
        }
        "list_dir" => ("noctalia.listDir", HostKind::Pair),
        "mkdir_all" => ("noctalia.mkdirAll", HostKind::Pair),
        "remove_file" => ("noctalia.removeFile", HostKind::Pair),
        "rename_file" => ("noctalia.renameFile", HostKind::Pair),
        "file_info" => ("noctalia.fileInfo", HostKind::Value),
        "env" => ("noctalia.getenv", HostKind::Value),
        "now_ms" => ("noctalia.nowMs", HostKind::Handler),
        "plugin_dir" => ("noctalia.pluginDir", HostKind::Value),
        "plugin_data_dir" => ("noctalia.pluginDataDir", HostKind::Pair),
        "color" => ("noctalia.getColor", HostKind::Handler),
        "clipboard" => ("noctalia.clipboardText", HostKind::Value),
        "set_op_sink" => return None,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EntryKind, NativeMode};

    fn options() -> Options<'static> {
        Options {
            handler: "handler",
            entry: "widget",
            state_key: "hello.state",
            kind: EntryKind::Widget,
            strings: &[],
            settings: &[],
            module: "main",
            modules: &[],
            native: NativeMode::Deny,
            strict: None,
        }
    }

    fn source(handler: &str) -> String {
        format!(
            "use noctalia_rs::{{Context, Op, Result}};\nuse serde_json::json;\n\n{handler}\n"
        )
    }

    fn lower_handler(handler: &str) -> IrProgram {
        let source = source(handler);
        lower(&source, &options()).expect("lower")
    }

    fn has_effect(program: &IrProgram, effect: &Effect) -> bool {
        program.effects.iter().any(|found| found == effect)
    }

    #[test]
    fn spans_follow_the_source() {
        let program = lower_handler(
            "fn handler(_ctx: &Context) -> Result<Vec<Op>> {\n    let x = 1u8;\n    Ok(vec![])\n}\n",
        );
        assert_eq!(program.span.file, "main");
        assert_eq!(program.span.line, 4);
        assert!(program.span.column > 0);
        let let_statement = &program.functions[0].body.stmts[0];
        assert_eq!(let_statement.span.line, 5);
        assert_eq!(
            let_statement.span.render(),
            format!("main:5:{}", let_statement.span.column)
        );
    }

    #[test]
    fn host_effects_keep_the_api57_marker_and_return_type() {
        let program = lower_handler(
            r#"fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = noctalia::getSetting("key");
    let (text, error) = noctalia::writeFile("/a", "x");
    Ok(vec![Op::log(text)])
}"#,
        );
        assert!(has_effect(
            &program,
            &Effect::Host {
                path: "noctalia.getSetting".to_owned(),
                kind: HostKind::Handler,
            }
        ));
        assert!(has_effect(
            &program,
            &Effect::Host {
                path: "noctalia.writeFile".to_owned(),
                kind: HostKind::Pair,
            }
        ));
        let StmtKind::Let { value, .. } =
            &program.functions[0].body.stmts[1].kind
        else {
            panic!("expected a let");
        };
        assert_eq!(
            value.ty,
            Ty::Tuple(vec![Ty::Value, Ty::Value]),
            "the ABI pair must stay typed"
        );
    }

    #[test]
    fn context_methods_resolve_to_host_effects() {
        let program = lower_handler(
            r#"fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let exists = ctx.file_exists("/a");
    let now = ctx.now_ms();
    Ok(vec![Op::log(exists.to_string()), Op::log(now)])
}"#,
        );
        assert!(has_effect(
            &program,
            &Effect::Host {
                path: "noctalia.fileExists".to_owned(),
                kind: HostKind::Value,
            }
        ));
        assert!(has_effect(
            &program,
            &Effect::Host {
                path: "noctalia.nowMs".to_owned(),
                kind: HostKind::Handler,
            }
        ));
    }

    #[test]
    fn callbacks_and_allocations_are_separate_effects() {
        let program = lower_handler(
            r#"fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let id = ui::callback("onRow");
    let mut ops = Vec::new();
    ops.push(Op::log(id));
    Ok(ops)
}"#,
        );
        assert!(has_effect(
            &program,
            &Effect::Callback {
                id: "ui::callback".to_owned(),
            }
        ));
        assert!(has_effect(
            &program,
            &Effect::Allocate {
                kind: AllocKind::Vec,
            }
        ));
        assert!(has_effect(
            &program,
            &Effect::Allocate {
                kind: AllocKind::Op,
            }
        ));
    }

    #[test]
    fn native_effects_fail_closed_in_strict() {
        let source = source(
            r#"fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let _ = std::fs::read("/a");
    Ok(vec![])
}"#,
        );
        let error =
            lower_strict(&source, &options()).expect_err("native effect");
        assert!(error.message.contains("native effect"), "{error}");
        assert_eq!(
            error.render("widget.rs"),
            "main:5:13: native effect in strict mode: call to `std::fs::read` needs native code"
        );
    }

    #[test]
    fn auto_allows_native_effects() {
        let program = lower_handler(
            r#"fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let _ = std::fs::read("/a");
    Ok(vec![])
}"#,
        );
        validate(&program, Mode::Auto).expect("auto keeps the native boundary");
        validate(&program, Mode::Off).expect("off ignores the IR");
    }

    #[test]
    fn null_in_an_array_slot_fails_closed() {
        let source = source(
            r#"fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let values = [json!(null)];
    Ok(vec![Op::log(values.len())])
}"#,
        );
        let error = lower(&source, &options()).expect_err("null slot");
        assert!(error.message.contains("JSON null"), "{error}");
        assert!(error.render("widget.rs").starts_with("main:5:"), "{error}");
    }

    #[test]
    fn integers_keep_width_and_signedness() {
        let program = lower_handler(
            r#"fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let small = 7u8;
    let signed = -3i16;
    Ok(vec![Op::log(small), Op::log(signed)])
}"#,
        );
        let small = &program.functions[0].body.stmts[0];
        let StmtKind::Let { value, .. } = &small.kind else {
            panic!("expected a let");
        };
        assert_eq!(
            value.ty,
            Ty::Int(IntType {
                signed: false,
                width: 8,
            })
        );
        let signed = &program.functions[0].body.stmts[1];
        let StmtKind::Let { value, .. } = &signed.kind else {
            panic!("expected a let");
        };
        assert_eq!(
            value.ty,
            Ty::Int(IntType {
                signed: true,
                width: 16,
            })
        );
    }

    #[test]
    fn mixed_integer_widths_fail_closed() {
        let source = source(
            r#"fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = 1u8 + 2i16;
    Ok(vec![Op::log(value)])
}"#,
        );
        let error = lower(&source, &options()).expect_err("mixed widths");
        assert!(error.message.contains("width or signedness"), "{error}");
        assert!(error.render("widget.rs").starts_with("main:5:"), "{error}");
    }

    #[test]
    fn out_of_range_literals_fail_closed() {
        let source = source(
            r#"fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = 256u8;
    Ok(vec![Op::log(value)])
}"#,
        );
        let error = lower(&source, &options()).expect_err("literal edge");
        assert!(error.message.contains("does not fit"), "{error}");
        assert!(error.render("widget.rs").starts_with("main:5:"), "{error}");
    }

    #[test]
    fn shadowed_bindings_get_distinct_ids() {
        let program = lower_handler(
            r#"fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let x = 1u8;
    let x = "text";
    Ok(vec![Op::log(x)])
}"#,
        );
        let first = &program.functions[0].body.stmts[0];
        let second = &program.functions[0].body.stmts[1];
        let (
            StmtKind::Let { binding: a, .. },
            StmtKind::Let { binding: b, .. },
        ) = (&first.kind, &second.kind)
        else {
            panic!("expected two lets");
        };
        assert_ne!(a.id, b.id, "a shadowing let is a new binding");
        assert_eq!(
            a.ty,
            Ty::Int(IntType {
                signed: false,
                width: 8
            })
        );
        assert_eq!(b.ty, Ty::Str);
    }

    #[test]
    fn operands_keep_source_evaluation_order() {
        let program = lower_handler(
            r#"fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let value = ctx.now_ms() + ctx.now_ms();
    Ok(vec![Op::log(value)])
}"#,
        );
        let StmtKind::Let { value, .. } =
            &program.functions[0].body.stmts[0].kind
        else {
            panic!("expected a let");
        };
        let ExprKind::Binary { left, right, .. } = &value.kind else {
            panic!("expected a binary");
        };
        let (
            ExprKind::Call {
                target: left_target,
                ..
            },
            ExprKind::Call {
                target: right_target,
                ..
            },
        ) = (&left.kind, &right.kind)
        else {
            panic!("expected two calls");
        };
        assert_eq!(left_target, &CallTarget::Method("now_ms".to_owned()));
        assert_eq!(right_target, &CallTarget::Method("now_ms".to_owned()));
        assert!(
            left.span.line < right.span.line
                || left.span.column < right.span.column
        );
    }

    #[test]
    fn unknown_api_members_fail_closed() {
        let source = source(
            r#"fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let _ = noctalia::notAnApiMember();
    Ok(vec![])
}"#,
        );
        let error = lower(&source, &options()).expect_err("unknown member");
        assert!(error.message.contains("unknown API member"), "{error}");
        assert!(error.render("widget.rs").starts_with("main:5:"), "{error}");
    }

    #[test]
    fn panic_macros_are_native_effects() {
        let program = lower_handler(
            r#"fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    panic!("stop");
}"#,
        );
        assert!(has_effect(
            &program,
            &Effect::Native {
                reason: "panic macro `panic`".to_owned(),
            }
        ));
        let source = source(
            r#"fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    panic!("stop");
}"#,
        );
        assert!(lower_strict(&source, &options()).is_err());
    }
    #[test]
    fn config_reads_keep_the_getconfig_marker() {
        let program = lower_handler(
            r#"fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let text = ctx.config_str("theme.name");
    let flag = ctx.config_bool("theme.dark", false);
    Ok(vec![Op::log(text), Op::log(flag)])
}"#,
        );
        assert!(has_effect(
            &program,
            &Effect::Host {
                path: "noctalia.getConfig".to_owned(),
                kind: HostKind::Handler,
            }
        ));
        assert!(!has_effect(
            &program,
            &Effect::Host {
                path: "noctalia.getSetting".to_owned(),
                kind: HostKind::Handler,
            }
        ));
    }

    #[test]
    fn nested_json_null_fails_closed() {
        for body in [
            r#"let _ = json!({"a": null});"#,
            r#"let _ = json!([null]);"#,
            r#"let _ = Op::query("q", "n", &["a"], Vec::new(), json!(null));"#,
            r#"let mut value = json!({}); value = json!(null);"#,
            r#"let _ = (json!(null), 1u8);"#,
        ] {
            let source = source(&format!(
                "fn handler(_ctx: &Context) -> Result<Vec<Op>> {{ {body} Ok(vec![]) }}"
            ));
            let error = lower(&source, &options()).expect_err(body);
            assert!(error.message.contains("JSON null"), "{body}: {error}");
            assert!(error.line > 0 && error.column > 0, "{body}: {error}");
        }
    }

    #[test]
    fn unresolved_calls_and_methods_fail_closed() {
        for (body, message) in [
            ("let _ = unknown_function();", "no proven Luau semantics"),
            (
                "let _ = values.unknownMethod();",
                "no proven Luau semantics",
            ),
            ("let _ = ctx.noSuchMethod();", "no proven API 57 member"),
        ] {
            let source = source(&format!(
                "fn handler(ctx: &Context) -> Result<Vec<Op>> {{ let values = Vec::new(); {body} Ok(vec![]) }}"
            ));
            let error = lower(&source, &options()).expect_err(body);
            assert!(error.message.contains(message), "{body}: {error}");
            assert!(error.line > 0 && error.column > 0, "{body}: {error}");
        }
    }

    #[test]
    fn host_backed_std_calls_are_not_native() {
        let program = lower_handler(
            r#"fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let setting = std::env::var("HOME");
    let now = std::time::SystemTime::now();
    Ok(vec![Op::log(setting), Op::log(now)])
}"#,
        );
        assert!(has_effect(
            &program,
            &Effect::Host {
                path: "noctalia.getenv".to_owned(),
                kind: HostKind::Value,
            }
        ));
        assert!(has_effect(
            &program,
            &Effect::Host {
                path: "noctalia.nowMs".to_owned(),
                kind: HostKind::Handler,
            }
        ));
        validate(&program, Mode::Strict)
            .expect("API 57 backed calls stay strict");
    }

    #[test]
    fn integer_boundaries_keep_exact_width() {
        let program = lower_handler(
            r#"fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let low = -128i8;
    let high = 340282366920938463463374607431768211455u128;
    Ok(vec![Op::log(low), Op::log(high)])
}"#,
        );
        assert!(program.functions[0].body.stmts.len() >= 2);
        for (body, message) in [
            ("let value = 128i8;", "does not fit"),
            ("let value = -1u8;", "negating an unsigned"),
            ("let value = b\"bytes\";", "byte string"),
        ] {
            let source = source(&format!(
                "fn handler(_ctx: &Context) -> Result<Vec<Op>> {{ {body} Ok(vec![]) }}"
            ));
            let error = lower(&source, &options()).expect_err(body);
            assert!(error.message.contains(message), "{body}: {error}");
        }
        assert!(IntType::I64.fits("9223372036854775807"));
        assert!(!IntType::I64.fits("9223372036854775808"));
        assert!(
            IntType {
                signed: false,
                width: 128
            }
            .fits("340282366920938463463374607431768211455")
        );
    }

    #[test]
    fn helper_spans_name_their_module() {
        let helper = "fn helper() -> i64 { 1 }\n";
        let source = "use noctalia_rs::{Context, Op, Result};\n\nfn handler(_ctx: &Context) -> Result<Vec<Op>> { let _ = widget::helper(); Ok(vec![]) }\n".to_owned();
        let modules = [crate::Module {
            path: "widget",
            source: helper,
        }];
        let mut options = options();
        options.modules = &modules;
        let program = lower(&source, &options).expect("lower helper module");
        let helper = program
            .functions
            .iter()
            .find(|function| function.name == "widget::helper")
            .expect("helper function");
        assert_eq!(helper.span.file, "widget");
        assert!(helper.span.line > 0 && helper.span.column > 0);
    }

    #[test]
    fn early_return_and_bytes_fail_closed() {
        for (body, message) in [
            ("let value = read_value()?;", "early return path"),
            ("let value = b\"abc\";", "byte string"),
            ("loop {}", "not proven"),
            ("while true {}", "not proven"),
            ("match 1 { _ => 2 }", "not proven"),
            ("struct Bad {}", "item statement"),
            ("unknown_macro!();", "no proven Luau semantics"),
        ] {
            let source = source(&format!(
                "fn handler(_ctx: &Context) -> Result<Vec<Op>> {{ {body} Ok(vec![]) }}"
            ));
            let error = lower(&source, &options()).expect_err(body);
            assert!(error.message.contains(message), "{body}: {error}");
            assert!(error.line > 0 && error.column > 0, "{body}: {error}");
        }
    }

    #[test]
    fn helper_return_type_is_exact() {
        let program = lower_handler(
            r#"fn helper() -> u8 { 1u8 }
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = helper() + 2u8;
    Ok(vec![Op::log(value)])
}"#,
        );
        let StmtKind::Let { value, .. } =
            &program.functions[1].body.stmts[0].kind
        else {
            panic!("expected a let");
        };
        assert_eq!(
            value.ty,
            Ty::Int(IntType {
                signed: false,
                width: 8,
            })
        );
    }

    #[test]
    fn kitchen_sink_lowering_keeps_forms_and_effects() {
        let program = lower_handler(
            r#"fn helper(value: i64) -> i64 { value + 1 }
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let mut total = helper(1);
    total = total + 2;
    let bits = (total & 1) | (total ^ 2);
    let shifted = bits << 1 >> 1;
    let comparisons = total == 1 || total != 2 && total < 3 && total <= 4 && total > 0 && total >= 0;
    let text = format!("{}", total);
    let chars = ['a', 'b'];
    let repeated = [0u8; 2];
    let tuple = (1u8, "x");
    let indexed = chars[0];
    let casted = total as i64;
    let range = 1..3;
    let closure = |x: i64| x + 1;
    let closure_value = closure(2);
    let branch = if comparisons { 1 } else if total == 0 { 2 } else { 3 };
    let negated = -total;
    let inverted = !comparisons;
    let path = &text;
    let deref = *path;
    let _ = (shifted, tuple, indexed, casted, range, closure_value, branch, negated, inverted, deref, repeated);
    if ctx.event == "action" { return Ok(vec![]); }
    Ok(vec![Op::log(text)])
}"#,
        );
        assert_eq!(program.functions.len(), 2);
        assert!(has_effect(
            &program,
            &Effect::Allocate {
                kind: AllocKind::Str,
            }
        ));
    }

    #[test]
    fn every_api_marker_kind_has_a_typed_result() {
        use crate::strict::MemberKind;
        for (member, kind, ty) in [
            ("noctalia::setWallpaperEnabled", MemberKind::Void, Ty::Unit),
            ("noctalia::runInTerminal", MemberKind::Bool, Ty::Bool),
            (
                "noctalia::writeFile",
                MemberKind::Pair,
                Ty::Tuple(vec![Ty::Value, Ty::Value]),
            ),
            ("noctalia::getSetting", MemberKind::Handler, Ty::Dynamic),
            ("ui::column", MemberKind::Node, Ty::Node),
        ] {
            let source = source(&format!(
                "fn handler(_ctx: &Context) -> Result<Vec<Op>> {{ let _ = {member}(); Ok(vec![]) }}"
            ));
            let program = lower(&source, &options()).expect(member);
            assert!(has_effect(
                &program,
                &Effect::Host {
                    path: member.replace("::", "."),
                    kind: HostKind::from(kind),
                }
            ));
            let StmtKind::Let { value, .. } =
                &program.functions[0].body.stmts[0].kind
            else {
                panic!("expected a let");
            };
            assert_eq!(value.ty, ty, "{member}");
        }
    }
}
