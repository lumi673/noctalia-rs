use std::fmt::Write as _;

use super::ast::{
    Block, Branch, Expr, ExprKind, Function, Script, Stmt, StmtKind,
    TableField, TableFieldKind,
};

pub(crate) fn print_script(script: &Script) -> String {
    let mut output = String::new();
    for statement in &script.statements {
        output.push_str(&statement.leading);
        print_stmt(statement, &mut output, 0);
        output.push_str(&statement.trailing);
    }
    output.push_str(&script.trailing);
    output
}

fn print_stmt(statement: &Stmt, output: &mut String, depth: usize) {
    if let StmtKind::Raw(text) = &statement.kind {
        output.push_str(text);
        return;
    }
    if let Some(text) = &statement.original {
        output.push_str(text);
        return;
    }
    match &statement.kind {
        StmtKind::Local { names, values } => {
            output.push_str("local ");
            output.push_str(&names.join(", "));
            if !values.is_empty() {
                output.push_str(" = ");
                print_expressions(values, output);
            }
        }
        StmtKind::LocalFunction { function } => {
            output.push_str("local function ");
            output.push_str(&function.name);
            print_function(function, output, depth);
        }
        StmtKind::Function { target, function } => {
            output.push_str("function ");
            print_expr(target, output);
            print_function(function, output, depth);
        }
        StmtKind::Assign { targets, values } => {
            print_expressions(targets, output);
            output.push_str(" = ");
            print_expressions(values, output);
        }
        StmtKind::Call(expr) => print_expr(expr, output),
        StmtKind::Return(values) => {
            output.push_str("return");
            if !values.is_empty() {
                output.push(' ');
                print_expressions(values, output);
            }
        }
        StmtKind::Break => output.push_str("break"),
        StmtKind::If {
            branches,
            else_body,
        } => {
            print_if(branches, else_body.as_ref(), output, depth);
        }
        StmtKind::Do(body) => {
            output.push_str("do\n");
            print_block(body, "end", output, depth.saturating_add(1));
        }
        StmtKind::While {
            lead,
            condition,
            tail,
            body,
        } => {
            output.push_str("while");
            output.push_str(lead);
            print_expr(condition, output);
            output.push_str(tail);
            output.push_str("do\n");
            print_block(body, "end", output, depth.saturating_add(1));
        }
        StmtKind::Repeat { body, condition } => {
            output.push_str("repeat\n");
            print_block(body, "", output, depth.saturating_add(1));
            output.push_str(" until ");
            print_expr(condition, output);
        }
        StmtKind::NumericFor {
            lead,
            name,
            start,
            limit,
            step,
            tail,
            body,
        } => {
            output.push_str("for");
            output.push_str(lead);
            output.push_str(name);
            output.push_str(" = ");
            print_expr(start, output);
            output.push_str(", ");
            print_expr(limit, output);
            if let Some(step) = step {
                output.push_str(", ");
                print_expr(step, output);
            }
            output.push_str(tail);
            output.push_str("do\n");
            print_block(body, "end", output, depth.saturating_add(1));
        }
        StmtKind::GenericFor {
            lead,
            names,
            iterator,
            tail,
            body,
        } => {
            output.push_str("for");
            output.push_str(lead);
            output.push_str(&names.join(", "));
            output.push_str(" in ");
            print_expr(iterator, output);
            output.push_str(tail);
            output.push_str("do\n");
            print_block(body, "end", output, depth.saturating_add(1));
        }
        StmtKind::Raw(_) => {}
    }
}

fn print_if(
    branches: &[Branch],
    else_body: Option<&Block>,
    output: &mut String,
    depth: usize,
) {
    for branch in branches {
        output.push_str(&branch.keyword);
        output.push_str(&branch.lead);
        if !output.ends_with(' ') {
            output.push(' ');
        }
        print_expr(&branch.condition, output);
        output.push_str(&branch.tail);
        output.push_str("then\n");
        print_block(&branch.body, "", output, depth.saturating_add(1));
    }
    if let Some(body) = else_body {
        output.push_str("else\n");
        print_block(body, "", output, depth.saturating_add(1));
    }
    if output.ends_with('\n') {
        output.push_str(&"  ".repeat(depth));
    }
    output.push_str("end");
}

fn print_block(
    block: &Block,
    terminator: &str,
    output: &mut String,
    depth: usize,
) {
    for statement in &block.statements {
        if output.ends_with('\n') {
            output.push_str(&"  ".repeat(depth));
        }
        output.push_str(
            statement
                .leading
                .trim_start_matches([' ', '\t', '\r', '\n']),
        );
        print_stmt(statement, output, depth);
        output.push_str(&statement.trailing);
        if !output.ends_with('\n') {
            output.push('\n');
        }
    }
    if output.ends_with('\n') {
        output.push_str(&"  ".repeat(depth.saturating_sub(1)));
    }
    output.push_str(
        block
            .close_leading
            .trim_start_matches([' ', '\t', '\r', '\n']),
    );
    if !output.ends_with('\n') {
        output.push('\n');
    }
    if !terminator.is_empty() {
        output.push_str(terminator);
    }
}

fn print_function(function: &Function, output: &mut String, depth: usize) {
    if let Some(text) = &function.original {
        output.push_str(text);
        return;
    }
    output.push('(');
    for (index, param) in function.params.iter().enumerate() {
        if index > 0 {
            output.push_str(", ");
        }
        output.push_str(param);
    }
    if function.varargs {
        if !function.params.is_empty() {
            output.push_str(", ");
        }
        output.push_str("...");
    }
    output.push(')');
    let inline = !function.body.statements.is_empty()
        && function
            .body
            .statements
            .iter()
            .all(|statement| matches!(statement.kind, StmtKind::Call(_)));
    if inline {
        output.push(' ');
        let last = function.body.statements.len().saturating_sub(1);
        for (index, statement) in function.body.statements.iter().enumerate() {
            print_stmt(statement, output, depth.saturating_add(1));
            if index != last {
                if !output.ends_with(';') {
                    output.push(';');
                }
                output.push(' ');
            }
        }
        output.push_str(" end");
    } else {
        output.push('\n');
        print_block(&function.body, "end", output, depth.saturating_add(1));
    }
}

fn print_expressions(expressions: &[Expr], output: &mut String) {
    for (index, expression) in expressions.iter().enumerate() {
        if index > 0 {
            output.push_str(", ");
        }
        print_expr(expression, output);
    }
}

fn print_expr(expression: &Expr, output: &mut String) {
    if let ExprKind::Raw(text) = &expression.kind {
        output.push_str(text);
        return;
    }
    if let Some(text) = &expression.original {
        output.push_str(text);
        return;
    }
    match &expression.kind {
        ExprKind::Nil => output.push_str("nil"),
        ExprKind::Bool(true) => output.push_str("true"),
        ExprKind::Bool(false) => output.push_str("false"),
        ExprKind::Integer(value) => {
            let _ = write!(output, "{}", value);
        }
        ExprKind::Float(value) => output.push_str(value),
        ExprKind::String(value) => quote(value, output),
        ExprKind::Varargs => output.push_str("..."),
        ExprKind::Var(name) => output.push_str(name),
        ExprKind::Field {
            object,
            field,
            separator,
        } => {
            print_expr(object, output);
            output.push_str(separator);
            output.push_str(field);
        }
        ExprKind::Index { object, index } => {
            print_expr(object, output);
            output.push('[');
            print_expr(index, output);
            output.push(']');
        }
        ExprKind::Unary { op, operand } => {
            if op == "not" {
                output.push_str("not ");
            } else {
                output.push_str(op);
            }
            print_expr(operand, output);
        }
        ExprKind::Binary { lhs, op, rhs } => {
            print_expr(lhs, output);
            output.push(' ');
            output.push_str(op);
            output.push(' ');
            print_expr(rhs, output);
        }
        ExprKind::Paren(inner) => {
            output.push('(');
            print_expr(inner, output);
            output.push(')');
        }
        ExprKind::Call { callee, args } => {
            print_expr(callee, output);
            output.push('(');
            print_expressions(args, output);
            output.push(')');
        }
        ExprKind::MethodCall {
            object,
            method,
            args,
        } => {
            print_expr(object, output);
            output.push(':');
            output.push_str(method);
            output.push('(');
            print_expressions(args, output);
            output.push(')');
        }
        ExprKind::Table(fields) => {
            output.push('{');
            for field in fields {
                print_field(field, output);
            }
            output.push('}');
        }
        ExprKind::Function(function) => {
            output.push_str("function");
            print_function(function, output, 0);
        }
        ExprKind::Raw(_) => {}
    }
}

fn print_field(field: &TableField, output: &mut String) {
    if let Some(text) = &field.original {
        output.push_str(&field.leading);
        output.push_str(text);
        output.push_str(&field.separator);
        return;
    }
    output.push_str(&field.leading);
    match &field.kind {
        TableFieldKind::Named(name, value) => {
            output.push_str(name);
            output.push_str(" = ");
            print_expr(value, output);
        }
        TableFieldKind::Keyed(key, value) => {
            output.push('[');
            print_expr(key, output);
            output.push_str("] = ");
            print_expr(value, output);
        }
        TableFieldKind::Positional(value) => print_expr(value, output),
    }
    output.push_str(&field.separator);
}

fn quote(value: &str, output: &mut String) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            other if other.is_control() => {
                let _ = write!(output, "\\{:03}", u32::from(other));
            }
            other => output.push(other),
        }
    }
    output.push('"');
}
