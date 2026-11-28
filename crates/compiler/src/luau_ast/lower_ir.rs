//! Lowers the typed IR into the Luau AST.
//!
//! Every node keeps the source span from the IR. A construct with no proven
//! Luau form fails closed at `file:line:column`; it is never guessed.

use crate::Error;
use crate::ir::{
    BinaryOp, Binding, Block as IrBlock, CallTarget, Expr as IrExpr,
    ExprKind as IrExprKind, Function as IrFunction, IrProgram, Literal,
    Span as IrSpan, Stmt as IrStmt, StmtKind as IrStmtKind, UnaryOp,
};

use super::ast::{
    Block, Branch, Expr, ExprKind, Function, Script, Span, Stmt, StmtKind,
    TableField, TableFieldKind,
};

fn span(value: &IrSpan) -> Span {
    Span::new(Some(value.file.clone()), value.line, value.column)
}

fn error(value: &IrSpan, message: impl Into<String>) -> Error {
    Error {
        message: message.into(),
        line: value.line,
        column: value.column,
        file: Some(value.file.clone()),
        fatal: false,
    }
}

fn expr(value: &IrSpan, kind: ExprKind) -> Expr {
    Expr {
        span: span(value),
        kind,
        original: None,
    }
}

fn stmt(value: &IrSpan, kind: StmtKind) -> Stmt {
    Stmt {
        span: span(value),
        kind,
        leading: String::new(),
        trailing: String::new(),
        original: None,
    }
}

fn block(value: &IrSpan, statements: Vec<Stmt>) -> Block {
    Block {
        span: span(value),
        statements,
        close_leading: String::new(),
    }
}

fn binding_name(binding: &Binding) -> String {
    format!("{}_{}", binding.name, binding.id)
}

fn short_name(path: &str) -> String {
    path.rsplit("::").next().unwrap_or(path).to_owned()
}

fn path_text(path: &str) -> String {
    path.split("::").collect::<Vec<_>>().join(".")
}

fn binary_operator(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Rem => "%",
        BinaryOp::BitAnd => "&",
        BinaryOp::BitOr => "|",
        BinaryOp::BitXor => "~",
        BinaryOp::Shl => "<<",
        BinaryOp::Shr => ">>",
        BinaryOp::Eq => "==",
        BinaryOp::Ne => "~=",
        BinaryOp::Lt => "<",
        BinaryOp::Le => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::Ge => ">=",
        BinaryOp::And => "and",
        BinaryOp::Or => "or",
    }
}

fn positional_fields(values: Vec<Expr>, value: &IrSpan) -> Expr {
    expr(
        value,
        ExprKind::Table(
            values
                .into_iter()
                .map(|item| TableField {
                    span: item.span.clone(),
                    kind: TableFieldKind::Positional(item),
                    leading: String::new(),
                    separator: ", ".to_owned(),
                    original: None,
                })
                .collect(),
        ),
    )
}

fn lower_expr(value: &IrExpr) -> Result<Expr, Error> {
    let lowered = match &value.kind {
        IrExprKind::Literal(Literal::Str(text)) => {
            ExprKind::String(text.clone())
        }
        IrExprKind::Literal(Literal::Int { text, .. }) => {
            match text.parse::<i128>() {
                Ok(number) => ExprKind::Integer(number),
                Err(_) => {
                    return Err(error(
                        &value.span,
                        "integer literal has no exact Luau representation",
                    ));
                }
            }
        }
        IrExprKind::Literal(Literal::Float(text)) => {
            ExprKind::Float(text.clone())
        }
        IrExprKind::Literal(Literal::Bool(flag)) => ExprKind::Bool(*flag),
        IrExprKind::Literal(Literal::Null) => {
            return Err(error(
                &value.span,
                "JSON null has no exact Luau container value",
            ));
        }
        IrExprKind::Binding(binding) => ExprKind::Var(binding_name(binding)),
        IrExprKind::Path(path) => ExprKind::Var(path_text(path)),
        IrExprKind::Call { target, args } => {
            let mut arguments = Vec::new();
            for argument in args {
                arguments.push(lower_expr(argument)?);
            }
            let callee = match target {
                CallTarget::Path(path) => {
                    expr(&value.span, ExprKind::Var(path_text(path)))
                }
                CallTarget::Method(name) => {
                    expr(&value.span, ExprKind::Var(path_text(name)))
                }
                CallTarget::Expr(target) => lower_expr(target)?,
            };
            ExprKind::Call {
                callee: Box::new(callee),
                args: arguments,
            }
        }
        IrExprKind::Field { base, name } => ExprKind::Field {
            object: Box::new(lower_expr(base)?),
            field: name.clone(),
            separator: ".".to_owned(),
        },
        IrExprKind::Index { base, index } => ExprKind::Index {
            object: Box::new(lower_expr(base)?),
            index: Box::new(lower_expr(index)?),
        },
        IrExprKind::Binary { op, left, right } => ExprKind::Binary {
            lhs: Box::new(lower_expr(left)?),
            op: binary_operator(*op).to_owned(),
            rhs: Box::new(lower_expr(right)?),
        },
        IrExprKind::Unary { op, operand } => match op {
            UnaryOp::Neg => ExprKind::Unary {
                op: "-".to_owned(),
                operand: Box::new(lower_expr(operand)?),
            },
            UnaryOp::Not => ExprKind::Unary {
                op: "not".to_owned(),
                operand: Box::new(lower_expr(operand)?),
            },
            UnaryOp::Deref | UnaryOp::Ref => {
                return Err(error(
                    &value.span,
                    "reference operation has no exact Luau value",
                ));
            }
        },
        IrExprKind::Cast { .. }
        | IrExprKind::Repeat { .. }
        | IrExprKind::Macro { .. } => {
            return Err(error(
                &value.span,
                "expression has no proven Luau lowering",
            ));
        }
        IrExprKind::Array(values) | IrExprKind::Tuple(values) => {
            let mut lowered = Vec::new();
            for item in values {
                lowered.push(lower_expr(item)?);
            }
            return Ok(positional_fields(lowered, &value.span));
        }
        IrExprKind::Table(fields) => {
            let mut lowered = Vec::new();
            for (name, item) in fields {
                lowered.push(TableField {
                    span: span(&item.span),
                    kind: TableFieldKind::Named(
                        name.clone(),
                        lower_expr(item)?,
                    ),
                    leading: String::new(),
                    separator: ", ".to_owned(),
                    original: None,
                });
            }
            ExprKind::Table(lowered)
        }
        IrExprKind::If { .. } => {
            return Err(error(
                &value.span,
                "if expression has no exact Luau value form",
            ));
        }
        IrExprKind::Block(_) => {
            return Err(error(
                &value.span,
                "block expression has no exact Luau value form",
            ));
        }
    };
    Ok(expr(&value.span, lowered))
}

fn lower_blocks(
    branches: &[(IrExpr, IrBlock)],
    otherwise: Option<&IrBlock>,
) -> Result<StmtKind, Error> {
    let mut lowered = Vec::new();
    for (index, (condition, body)) in branches.iter().enumerate() {
        lowered.push(Branch {
            span: span(&condition.span),
            keyword: if index == 0 { "if" } else { "elseif" }.to_owned(),
            lead: " ".to_owned(),
            condition: lower_expr(condition)?,
            tail: " ".to_owned(),
            body: lower_block(body)?,
        });
    }
    Ok(StmtKind::If {
        branches: lowered,
        else_body: otherwise.map(lower_block).transpose()?,
    })
}

fn lower_if_expr(value: &IrExpr) -> Result<StmtKind, Error> {
    let IrExprKind::If {
        branches,
        otherwise,
    } = &value.kind
    else {
        return Err(error(&value.span, "expected an if expression"));
    };
    lower_blocks(branches, otherwise.as_ref())
}

fn lower_block(value: &IrBlock) -> Result<Block, Error> {
    let mut statements = Vec::new();
    for statement in &value.stmts {
        if let IrStmtKind::Expr(expression) = &statement.kind
            && let IrExprKind::Block(inner) = &expression.kind
        {
            for nested in &inner.stmts {
                statements.push(lower_stmt(nested)?);
            }
        } else {
            statements.push(lower_stmt(statement)?);
        }
    }
    Ok(block(&value.span, statements))
}

fn lower_stmt(value: &IrStmt) -> Result<Stmt, Error> {
    let kind = match &value.kind {
        IrStmtKind::Let {
            binding,
            value: init,
        } => StmtKind::Local {
            names: vec![binding_name(binding)],
            values: vec![lower_expr(init)?],
        },
        IrStmtKind::Assign {
            target,
            value: init,
        } => StmtKind::Assign {
            targets: vec![lower_expr(target)?],
            values: vec![lower_expr(init)?],
        },
        IrStmtKind::Expr(value) => {
            if matches!(value.kind, IrExprKind::If { .. }) {
                lower_if_expr(value)?
            } else {
                let lowered = lower_expr(value)?;
                if !matches!(lowered.kind, ExprKind::Call { .. }) {
                    return Err(error(
                        &value.span,
                        "expression statement has no Luau call form",
                    ));
                }
                StmtKind::Call(lowered)
            }
        }
        IrStmtKind::Return(value) => StmtKind::Return(
            value
                .as_ref()
                .map(lower_expr)
                .transpose()?
                .into_iter()
                .collect(),
        ),
        IrStmtKind::If {
            branches,
            otherwise,
        } => lower_blocks(branches, otherwise.as_ref())?,
    };
    Ok(stmt(&value.span, kind))
}

fn lower_function(value: &IrFunction) -> Result<Function, Error> {
    Ok(Function {
        name: short_name(&value.name),
        span: span(&value.span),
        params: value.params.iter().map(binding_name).collect(),
        varargs: false,
        body: lower_block(&value.body)?,
        original: None,
    })
}

/// Lowers a typed program into a printable Luau AST.
pub(crate) fn lower_ir(program: &IrProgram) -> Result<Script, Error> {
    let mut statements = Vec::new();
    for function in &program.functions {
        if function.name == program.entry {
            continue;
        }
        statements.push(stmt(
            &function.span,
            StmtKind::LocalFunction {
                function: lower_function(function)?,
            },
        ));
    }
    let entry = program
        .functions
        .iter()
        .find(|function| function.name == program.entry)
        .ok_or_else(|| {
            error(&program.span, "typed program has no entry function")
        })?;
    let target = expr(&entry.span, ExprKind::Var(short_name(&entry.name)));
    statements.push(stmt(
        &entry.span,
        StmtKind::Function {
            target,
            function: lower_function(entry)?,
        },
    ));
    Ok(Script {
        file: Some(program.span.file.clone()),
        statements,
        trailing: String::new(),
    })
}
