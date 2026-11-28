use std::collections::BTreeSet;
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Span {
    pub(crate) file: Option<String>,
    pub(crate) line: usize,
    pub(crate) column: usize,
}

impl Span {
    pub(crate) fn new(
        file: Option<String>,
        line: usize,
        column: usize,
    ) -> Self {
        Self { file, line, column }
    }

    pub(crate) fn render(&self) -> String {
        match &self.file {
            Some(file) => format!("{}:{}:{}", file, self.line, self.column),
            None => format!("{}:{}", self.line, self.column),
        }
    }
}

impl fmt::Display for Span {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.render())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Script {
    pub(crate) file: Option<String>,
    pub(crate) statements: Vec<Stmt>,
    pub(crate) trailing: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Stmt {
    pub(crate) span: Span,
    pub(crate) kind: StmtKind,
    pub(crate) leading: String,
    pub(crate) trailing: String,
    pub(crate) original: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StmtKind {
    Local {
        names: Vec<String>,
        values: Vec<Expr>,
    },
    LocalFunction {
        function: Function,
    },
    Function {
        target: Expr,
        function: Function,
    },
    Assign {
        targets: Vec<Expr>,
        values: Vec<Expr>,
    },
    Call(Expr),
    Return(Vec<Expr>),
    Break,
    If {
        branches: Vec<Branch>,
        else_body: Option<Block>,
    },
    Do(Block),
    While {
        lead: String,
        condition: Expr,
        tail: String,
        body: Block,
    },
    Repeat {
        body: Block,
        condition: Expr,
    },
    NumericFor {
        lead: String,
        name: String,
        start: Expr,
        limit: Expr,
        step: Option<Expr>,
        tail: String,
        body: Block,
    },
    GenericFor {
        lead: String,
        names: Vec<String>,
        iterator: Expr,
        tail: String,
        body: Block,
    },
    Raw(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Branch {
    pub(crate) span: Span,
    pub(crate) keyword: String,
    pub(crate) lead: String,
    pub(crate) condition: Expr,
    pub(crate) tail: String,
    pub(crate) body: Block,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Block {
    pub(crate) span: Span,
    pub(crate) statements: Vec<Stmt>,
    pub(crate) close_leading: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Function {
    pub(crate) name: String,
    pub(crate) span: Span,
    pub(crate) params: Vec<String>,
    pub(crate) varargs: bool,
    pub(crate) body: Block,
    pub(crate) original: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Expr {
    pub(crate) span: Span,
    pub(crate) kind: ExprKind,
    pub(crate) original: Option<String>,
}

impl Expr {
    pub(crate) fn is_effect_free(&self) -> bool {
        match &self.kind {
            ExprKind::Nil
            | ExprKind::Bool(_)
            | ExprKind::Integer(_)
            | ExprKind::Float(_)
            | ExprKind::String(_)
            | ExprKind::Varargs
            | ExprKind::Var(_) => true,
            ExprKind::Paren(inner) | ExprKind::Unary { operand: inner, .. } => {
                inner.is_effect_free()
            }
            ExprKind::Binary { lhs, rhs, .. } => {
                lhs.is_effect_free() && rhs.is_effect_free()
            }
            ExprKind::Table(fields) => {
                fields.iter().all(TableField::is_effect_free)
            }
            ExprKind::Function(_) => true,
            ExprKind::Field { .. }
            | ExprKind::Index { .. }
            | ExprKind::Call { .. }
            | ExprKind::MethodCall { .. }
            | ExprKind::Raw(_) => false,
        }
    }

    pub(crate) fn is_constant(&self) -> bool {
        match &self.kind {
            ExprKind::Nil
            | ExprKind::Bool(_)
            | ExprKind::Integer(_)
            | ExprKind::Float(_)
            | ExprKind::String(_) => true,
            ExprKind::Paren(inner) | ExprKind::Unary { operand: inner, .. } => {
                inner.is_constant()
            }
            ExprKind::Binary { lhs, rhs, .. } => {
                lhs.is_constant() && rhs.is_constant()
            }
            ExprKind::Table(fields) => {
                fields.iter().all(TableField::is_constant)
            }
            ExprKind::Varargs
            | ExprKind::Var(_)
            | ExprKind::Field { .. }
            | ExprKind::Index { .. }
            | ExprKind::Call { .. }
            | ExprKind::MethodCall { .. }
            | ExprKind::Function(_)
            | ExprKind::Raw(_) => false,
        }
    }

    pub(crate) fn global_name(&self) -> Option<&str> {
        match &self.kind {
            ExprKind::Var(name) => Some(name),
            ExprKind::Paren(inner) => inner.global_name(),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ExprKind {
    Nil,
    Bool(bool),
    Integer(i128),
    Float(String),
    String(String),
    Varargs,
    Var(String),
    Field {
        object: Box<Expr>,
        field: String,
        separator: String,
    },
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
    },
    Unary {
        op: String,
        operand: Box<Expr>,
    },
    Binary {
        lhs: Box<Expr>,
        op: String,
        rhs: Box<Expr>,
    },
    Paren(Box<Expr>),
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    MethodCall {
        object: Box<Expr>,
        method: String,
        args: Vec<Expr>,
    },
    Table(Vec<TableField>),
    Function(Function),
    Raw(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TableField {
    pub(crate) span: Span,
    pub(crate) kind: TableFieldKind,
    pub(crate) leading: String,
    pub(crate) separator: String,
    pub(crate) original: Option<String>,
}

impl TableField {
    pub(crate) fn is_effect_free(&self) -> bool {
        match &self.kind {
            TableFieldKind::Named(_, value)
            | TableFieldKind::Positional(value) => value.is_effect_free(),
            TableFieldKind::Keyed(key, value) => {
                key.is_effect_free() && value.is_effect_free()
            }
        }
    }

    pub(crate) fn is_constant(&self) -> bool {
        match &self.kind {
            TableFieldKind::Named(_, value)
            | TableFieldKind::Positional(value) => value.is_constant(),
            TableFieldKind::Keyed(key, value) => {
                key.is_constant() && value.is_constant()
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TableFieldKind {
    Named(String, Expr),
    Keyed(Expr, Expr),
    Positional(Expr),
}

pub(crate) fn expr_names(expr: &Expr, names: &mut BTreeSet<String>) {
    match &expr.kind {
        ExprKind::Var(name) => {
            let _ = names.insert(name.clone());
        }
        ExprKind::Field { object, .. }
        | ExprKind::Unary {
            operand: object, ..
        } => {
            expr_names(object, names);
        }
        ExprKind::Index { object, index } => {
            expr_names(object, names);
            expr_names(index, names);
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            expr_names(lhs, names);
            expr_names(rhs, names);
        }
        ExprKind::Paren(inner) => expr_names(inner, names),
        ExprKind::Call { callee, args } => {
            expr_names(callee, names);
            for arg in args {
                expr_names(arg, names);
            }
        }
        ExprKind::MethodCall { object, args, .. } => {
            expr_names(object, names);
            for arg in args {
                expr_names(arg, names);
            }
        }
        ExprKind::Table(fields) => {
            for field in fields {
                match &field.kind {
                    TableFieldKind::Named(_, value)
                    | TableFieldKind::Positional(value) => {
                        expr_names(value, names);
                    }
                    TableFieldKind::Keyed(key, value) => {
                        expr_names(key, names);
                        expr_names(value, names);
                    }
                }
            }
        }
        ExprKind::Function(function) => {
            block_names(&function.body, names);
        }
        ExprKind::Nil
        | ExprKind::Bool(_)
        | ExprKind::Integer(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Varargs
        | ExprKind::Raw(_) => {}
    }
}

pub(crate) fn block_names(block: &Block, names: &mut BTreeSet<String>) {
    for statement in &block.statements {
        stmt_names(statement, names);
    }
}

pub(crate) fn stmt_names(statement: &Stmt, names: &mut BTreeSet<String>) {
    match &statement.kind {
        StmtKind::Local { values, .. } => {
            for value in values {
                expr_names(value, names);
            }
        }
        StmtKind::LocalFunction { function }
        | StmtKind::Function { function, .. } => {
            block_names(&function.body, names);
        }
        StmtKind::Assign { targets, values } => {
            for target in targets {
                expr_names(target, names);
            }
            for value in values {
                expr_names(value, names);
            }
        }
        StmtKind::Call(expr) => expr_names(expr, names),
        StmtKind::Return(values) => {
            for value in values {
                expr_names(value, names);
            }
        }
        StmtKind::Break => {}
        StmtKind::If {
            branches,
            else_body,
        } => {
            for branch in branches {
                expr_names(&branch.condition, names);
                block_names(&branch.body, names);
            }
            if let Some(body) = else_body {
                block_names(body, names);
            }
        }
        StmtKind::Do(body)
        | StmtKind::While { body, .. }
        | StmtKind::Repeat { body, .. }
        | StmtKind::NumericFor { body, .. }
        | StmtKind::GenericFor { body, .. } => {
            block_names(body, names);
        }
        StmtKind::Raw(_) => {}
    }
}
