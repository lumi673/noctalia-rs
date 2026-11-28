use std::collections::{BTreeMap, BTreeSet};

use super::ast::{
    Block, Branch, Expr, ExprKind, Function, Script, Span, Stmt, StmtKind,
    TableField, TableFieldKind, stmt_names,
};

const EXACT_LOWER: i128 = -9007199254740992i128;
const EXACT_UPPER: i128 = 9007199254740992i128;

fn in_exact_range(value: i128) -> bool {
    value >= EXACT_LOWER && value <= EXACT_UPPER
}

fn make_expr(span: &Span, kind: ExprKind) -> Expr {
    Expr {
        span: span.clone(),
        kind,
        original: None,
    }
}

fn make_stmt(
    span: &Span,
    kind: StmtKind,
    leading: String,
    trailing: String,
) -> Stmt {
    Stmt {
        span: span.clone(),
        kind,
        leading,
        trailing,
        original: None,
    }
}

pub(crate) fn fold_constants(script: &Script) -> Script {
    let mut result = script.clone();
    let mut statements = Vec::new();
    for statement in &script.statements {
        append_folded(statement, &mut statements);
    }
    result.statements = statements;
    result
}

fn append_folded(statement: &Stmt, output: &mut Vec<Stmt>) {
    for folded in fold_stmt(statement) {
        output.push(folded);
    }
}

fn fold_stmt(statement: &Stmt) -> Vec<Stmt> {
    let mut changed = false;
    let kind = match &statement.kind {
        StmtKind::Local { names, values } => {
            let (folded, yes) = fold_expr_list(values);
            changed |= yes;
            StmtKind::Local {
                names: names.clone(),
                values: folded,
            }
        }
        StmtKind::LocalFunction { function } => {
            let folded = fold_function(function);
            changed |= folded.is_some();
            StmtKind::LocalFunction {
                function: folded.unwrap_or_else(|| function.clone()),
            }
        }
        StmtKind::Function { target, function } => {
            let folded = fold_function(function);
            changed |= folded.is_some();
            StmtKind::Function {
                target: target.clone(),
                function: folded.unwrap_or_else(|| function.clone()),
            }
        }
        StmtKind::Assign { targets, values } => {
            let (folded_targets, targets_changed) = fold_expr_list(targets);
            let (folded_values, values_changed) = fold_expr_list(values);
            changed |= targets_changed || values_changed;
            StmtKind::Assign {
                targets: folded_targets,
                values: folded_values,
            }
        }
        StmtKind::Call(expr) => {
            let folded = fold_expr(expr);
            changed |= folded.is_some();
            StmtKind::Call(folded.unwrap_or_else(|| expr.clone()))
        }
        StmtKind::Return(values) => {
            let (folded, yes) = fold_expr_list(values);
            changed |= yes;
            StmtKind::Return(folded)
        }
        StmtKind::Break => StmtKind::Break,
        StmtKind::If {
            branches,
            else_body,
        } => {
            let mut folded_branches = Vec::new();
            for branch in branches {
                let folded_condition = fold_expr(&branch.condition);
                changed |= folded_condition.is_some();
                let folded_body = fold_block(&branch.body);
                changed |= folded_body.statements.len()
                    != branch.body.statements.len()
                    || folded_body.close_leading != branch.body.close_leading;
                folded_branches.push(Branch {
                    span: branch.span.clone(),
                    keyword: branch.keyword.clone(),
                    lead: branch.lead.clone(),
                    condition: folded_condition
                        .unwrap_or_else(|| branch.condition.clone()),
                    tail: branch.tail.clone(),
                    body: folded_body,
                });
            }
            let folded_else = else_body.as_ref().map(fold_block);
            changed |= folded_else.as_ref().is_some_and(|body| {
                body.statements.len()
                    != else_body.as_ref().map_or(0, |old| old.statements.len())
            });
            let mut selected = None;
            let mut all_false = true;
            for branch in &folded_branches {
                match &branch.condition.kind {
                    ExprKind::Bool(true) => {
                        selected = Some(branch.body.clone());
                        break;
                    }
                    ExprKind::Bool(false) => {}
                    _ => {
                        all_false = false;
                        break;
                    }
                }
            }
            if selected.is_none() && all_false {
                if let Some(body) = &folded_else {
                    selected = Some(body.clone());
                }
            }
            if let Some(body) = selected {
                let mut body = body;
                if let Some(first) = body.statements.first_mut() {
                    let mut leading = statement.leading.clone();
                    leading.push_str(&first.leading);
                    first.leading = leading;
                }
                if let Some(last) = body.statements.last_mut() {
                    last.trailing.push_str(&statement.trailing);
                }
                return vec![make_stmt(
                    &statement.span,
                    StmtKind::Do(body),
                    String::new(),
                    String::new(),
                )];
            }
            StmtKind::If {
                branches: folded_branches,
                else_body: folded_else,
            }
        }
        StmtKind::Do(body) => {
            let folded = fold_block(body);
            changed |= folded.statements.len() != body.statements.len();
            StmtKind::Do(folded)
        }
        StmtKind::While {
            lead,
            condition,
            tail,
            body,
        } => {
            let folded_condition = fold_expr(condition);
            changed |= folded_condition.is_some();
            let folded_body = fold_block(body);
            changed |= folded_body.statements.len() != body.statements.len();
            StmtKind::While {
                lead: lead.clone(),
                condition: folded_condition
                    .unwrap_or_else(|| condition.clone()),
                tail: tail.clone(),
                body: folded_body,
            }
        }
        StmtKind::Repeat { body, condition } => {
            let folded_body = fold_block(body);
            changed |= folded_body.statements.len() != body.statements.len();
            let folded_condition = fold_expr(condition);
            changed |= folded_condition.is_some();
            StmtKind::Repeat {
                body: folded_body,
                condition: folded_condition
                    .unwrap_or_else(|| condition.clone()),
            }
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
            let folded_start = fold_expr(start);
            let folded_limit = fold_expr(limit);
            let folded_step = step.as_ref().and_then(fold_expr);
            changed |= folded_start.is_some()
                || folded_limit.is_some()
                || folded_step.is_some()
                || step.is_some() != folded_step.is_some();
            let folded_body = fold_block(body);
            changed |= folded_body.statements.len() != body.statements.len();
            StmtKind::NumericFor {
                lead: lead.clone(),
                name: name.clone(),
                start: folded_start.unwrap_or_else(|| start.clone()),
                limit: folded_limit.unwrap_or_else(|| limit.clone()),
                step: folded_step.or_else(|| step.clone()),
                tail: tail.clone(),
                body: folded_body,
            }
        }
        StmtKind::GenericFor {
            lead,
            names,
            iterator,
            tail,
            body,
        } => {
            let folded_iterator = fold_expr(iterator);
            changed |= folded_iterator.is_some();
            let folded_body = fold_block(body);
            changed |= folded_body.statements.len() != body.statements.len();
            StmtKind::GenericFor {
                lead: lead.clone(),
                names: names.clone(),
                iterator: folded_iterator.unwrap_or_else(|| iterator.clone()),
                tail: tail.clone(),
                body: folded_body,
            }
        }
        StmtKind::Raw(text) => StmtKind::Raw(text.clone()),
    };
    if changed {
        vec![make_stmt(
            &statement.span,
            kind,
            statement.leading.clone(),
            statement.trailing.clone(),
        )]
    } else {
        vec![statement.clone()]
    }
}

fn fold_block(block: &Block) -> Block {
    let mut statements = Vec::new();
    for statement in &block.statements {
        append_folded(statement, &mut statements);
    }
    Block {
        span: block.span.clone(),
        statements,
        close_leading: block.close_leading.clone(),
    }
}

fn fold_function(function: &Function) -> Option<Function> {
    let folded = fold_block(&function.body);
    if folded == function.body {
        return None;
    }
    Some(Function {
        name: function.name.clone(),
        span: function.span.clone(),
        params: function.params.clone(),
        varargs: function.varargs,
        body: folded,
        original: None,
    })
}

fn fold_expr_list(expressions: &[Expr]) -> (Vec<Expr>, bool) {
    let mut folded = Vec::new();
    let mut changed = false;
    for expression in expressions {
        let result = fold_expr(expression);
        changed |= result.is_some();
        folded.push(result.unwrap_or_else(|| expression.clone()));
    }
    (folded, changed)
}

fn fold_expr(expression: &Expr) -> Option<Expr> {
    match &expression.kind {
        ExprKind::Nil
        | ExprKind::Bool(_)
        | ExprKind::Integer(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Varargs
        | ExprKind::Var(_)
        | ExprKind::Raw(_) => None,
        ExprKind::Unary { op, operand } => {
            let folded_operand = fold_expr(operand);
            let changed = folded_operand.is_some();
            let operand =
                folded_operand.unwrap_or_else(|| operand.as_ref().clone());
            if op == "-" {
                if let ExprKind::Integer(value) = &operand.kind {
                    if in_exact_range(*value) {
                        if let Some(negated) = value.checked_neg() {
                            return Some(make_expr(
                                &expression.span,
                                ExprKind::Integer(negated),
                            ));
                        }
                    }
                }
            }
            if op == "not" {
                if let ExprKind::Bool(value) = &operand.kind {
                    return Some(make_expr(
                        &expression.span,
                        ExprKind::Bool(!value),
                    ));
                }
            }
            if changed {
                return Some(make_expr(
                    &expression.span,
                    ExprKind::Unary {
                        op: op.clone(),
                        operand: Box::new(operand),
                    },
                ));
            }
            None
        }
        ExprKind::Binary { lhs, op, rhs } => {
            let folded_lhs = fold_expr(lhs);
            let folded_rhs = fold_expr(rhs);
            let changed = folded_lhs.is_some() || folded_rhs.is_some();
            let lhs = folded_lhs.unwrap_or_else(|| lhs.as_ref().clone());
            let rhs = folded_rhs.unwrap_or_else(|| rhs.as_ref().clone());
            if let Some(kind) = fold_binary(&lhs, op, &rhs) {
                return Some(make_expr(&expression.span, kind));
            }
            if changed {
                return Some(make_expr(
                    &expression.span,
                    ExprKind::Binary {
                        lhs: Box::new(lhs),
                        op: op.clone(),
                        rhs: Box::new(rhs),
                    },
                ));
            }
            None
        }
        ExprKind::Paren(inner) => {
            let folded = fold_expr(inner)?;
            if matches!(
                &folded.kind,
                ExprKind::Nil
                    | ExprKind::Bool(_)
                    | ExprKind::Integer(_)
                    | ExprKind::Float(_)
                    | ExprKind::String(_)
            ) {
                return Some(folded);
            }
            Some(make_expr(
                &expression.span,
                ExprKind::Paren(Box::new(folded)),
            ))
        }
        ExprKind::Field {
            object,
            field,
            separator,
        } => {
            let folded = fold_expr(object)?;
            Some(make_expr(
                &expression.span,
                ExprKind::Field {
                    object: Box::new(folded),
                    field: field.clone(),
                    separator: separator.clone(),
                },
            ))
        }
        ExprKind::Index { object, index } => {
            let folded_object = fold_expr(object);
            let folded_index = fold_expr(index);
            if folded_object.is_none() && folded_index.is_none() {
                return None;
            }
            Some(make_expr(
                &expression.span,
                ExprKind::Index {
                    object: Box::new(
                        folded_object
                            .unwrap_or_else(|| object.as_ref().clone()),
                    ),
                    index: Box::new(
                        folded_index.unwrap_or_else(|| index.as_ref().clone()),
                    ),
                },
            ))
        }
        ExprKind::Call { callee, args } => {
            let folded_callee = fold_expr(callee);
            let (folded_args, args_changed) = fold_expr_list(args);
            if folded_callee.is_none() && !args_changed {
                return None;
            }
            Some(make_expr(
                &expression.span,
                ExprKind::Call {
                    callee: Box::new(
                        folded_callee
                            .unwrap_or_else(|| callee.as_ref().clone()),
                    ),
                    args: folded_args,
                },
            ))
        }
        ExprKind::MethodCall {
            object,
            method,
            args,
        } => {
            let folded_object = fold_expr(object);
            let (folded_args, args_changed) = fold_expr_list(args);
            if folded_object.is_none() && !args_changed {
                return None;
            }
            Some(make_expr(
                &expression.span,
                ExprKind::MethodCall {
                    object: Box::new(
                        folded_object
                            .unwrap_or_else(|| object.as_ref().clone()),
                    ),
                    method: method.clone(),
                    args: folded_args,
                },
            ))
        }
        ExprKind::Table(fields) => {
            let mut changed = false;
            let mut result = Vec::new();
            for field in fields {
                let folded = fold_table_field(field);
                changed |= folded.is_some();
                result.push(folded.unwrap_or_else(|| field.clone()));
            }
            if changed {
                return Some(make_expr(
                    &expression.span,
                    ExprKind::Table(result),
                ));
            }
            None
        }
        ExprKind::Function(function) => fold_function(function).map(|folded| {
            make_expr(&expression.span, ExprKind::Function(folded))
        }),
    }
}

fn fold_table_field(field: &TableField) -> Option<TableField> {
    let (key, key_changed) = match &field.kind {
        TableFieldKind::Keyed(key, _) => {
            let folded = fold_expr(key);
            let changed = folded.is_some();
            (folded, changed)
        }
        _ => (None, false),
    };
    let value_expr = match &field.kind {
        TableFieldKind::Named(_, value)
        | TableFieldKind::Keyed(_, value)
        | TableFieldKind::Positional(value) => value,
    };
    let folded_value = fold_expr(value_expr);
    if !key_changed && folded_value.is_none() {
        return None;
    }
    let kind = match &field.kind {
        TableFieldKind::Named(name, _) => TableFieldKind::Named(
            name.clone(),
            folded_value.unwrap_or_else(|| value_expr.clone()),
        ),
        TableFieldKind::Keyed(original_key, _) => TableFieldKind::Keyed(
            key.unwrap_or_else(|| original_key.clone()),
            folded_value.unwrap_or_else(|| value_expr.clone()),
        ),
        TableFieldKind::Positional(_) => TableFieldKind::Positional(
            folded_value.unwrap_or_else(|| value_expr.clone()),
        ),
    };
    Some(TableField {
        span: field.span.clone(),
        kind,
        leading: field.leading.clone(),
        separator: field.separator.clone(),
        original: None,
    })
}

fn fold_binary(lhs: &Expr, operator: &str, rhs: &Expr) -> Option<ExprKind> {
    if let (ExprKind::Integer(left), ExprKind::Integer(right)) =
        (&lhs.kind, &rhs.kind)
    {
        if !in_exact_range(*left) || !in_exact_range(*right) {
            return None;
        }
        let integer = match operator {
            "+" => left.checked_add(*right),
            "-" => left.checked_sub(*right),
            "*" => left.checked_mul(*right),
            _ => None,
        };
        if let Some(value) = integer {
            if in_exact_range(value) {
                return Some(ExprKind::Integer(value));
            }
            return None;
        }
        let boolean = match operator {
            "==" => Some(left == right),
            "~=" => Some(left != right),
            "<" => Some(left < right),
            "<=" => Some(left <= right),
            ">" => Some(left > right),
            ">=" => Some(left >= right),
            _ => None,
        };
        return boolean.map(ExprKind::Bool);
    }
    if let (ExprKind::String(left), ExprKind::String(right)) =
        (&lhs.kind, &rhs.kind)
    {
        if operator == ".." {
            let mut joined =
                String::with_capacity(left.len().saturating_add(right.len()));
            joined.push_str(left);
            joined.push_str(right);
            return Some(ExprKind::String(joined));
        }
    }
    if let (ExprKind::Bool(left), ExprKind::Bool(right)) =
        (&lhs.kind, &rhs.kind)
    {
        return match operator {
            "and" => Some(ExprKind::Bool(*left && *right)),
            "or" => Some(ExprKind::Bool(*left || *right)),
            _ => None,
        };
    }
    None
}

pub(crate) fn remove_dead_code(script: &Script) -> Script {
    let mut used = BTreeSet::new();
    for statement in &script.statements {
        stmt_names(statement, &mut used);
    }
    let mut result = script.clone();
    let mut statements = Vec::new();
    let mut stopped = false;
    for statement in &script.statements {
        if stopped {
            break;
        }
        if matches!(statement.kind, StmtKind::Return(_)) {
            statements.push(statement.clone());
            stopped = true;
            continue;
        }
        if removable(statement, &used) {
            continue;
        }
        statements.push(clean_stmt(statement, &used));
    }
    result.statements = statements;
    result
}

fn removable(statement: &Stmt, used: &BTreeSet<String>) -> bool {
    match &statement.kind {
        StmtKind::Do(body) => body.statements.is_empty(),
        StmtKind::Local { names, values } => {
            names.iter().all(|name| !used.contains(name))
                && values.iter().all(Expr::is_effect_free)
        }
        StmtKind::LocalFunction { function } => {
            let name = function_local_name(function);
            !name.is_empty() && !used.contains(name)
        }
        _ => false,
    }
}

fn function_local_name(function: &Function) -> &str {
    &function.name
}

fn clean_stmt(statement: &Stmt, used: &BTreeSet<String>) -> Stmt {
    let kind = match &statement.kind {
        StmtKind::If {
            branches,
            else_body,
        } => {
            let mut new_branches = Vec::new();
            for branch in branches {
                new_branches.push(Branch {
                    span: branch.span.clone(),
                    keyword: branch.keyword.clone(),
                    lead: branch.lead.clone(),
                    condition: branch.condition.clone(),
                    tail: branch.tail.clone(),
                    body: clean_block(&branch.body, used),
                });
            }
            StmtKind::If {
                branches: new_branches,
                else_body: else_body
                    .as_ref()
                    .map(|body| clean_block(body, used)),
            }
        }
        StmtKind::Do(body) => StmtKind::Do(clean_block(body, used)),
        StmtKind::While {
            lead,
            condition,
            tail,
            body,
        } => StmtKind::While {
            lead: lead.clone(),
            condition: condition.clone(),
            tail: tail.clone(),
            body: clean_block(body, used),
        },
        StmtKind::Repeat { body, condition } => StmtKind::Repeat {
            body: clean_block(body, used),
            condition: condition.clone(),
        },
        StmtKind::NumericFor {
            lead,
            name,
            start,
            limit,
            step,
            tail,
            body,
        } => StmtKind::NumericFor {
            lead: lead.clone(),
            name: name.clone(),
            start: start.clone(),
            limit: limit.clone(),
            step: step.clone(),
            tail: tail.clone(),
            body: clean_block(body, used),
        },
        StmtKind::GenericFor {
            lead,
            names,
            iterator,
            tail,
            body,
        } => StmtKind::GenericFor {
            lead: lead.clone(),
            names: names.clone(),
            iterator: iterator.clone(),
            tail: tail.clone(),
            body: clean_block(body, used),
        },
        StmtKind::LocalFunction { function } => StmtKind::LocalFunction {
            function: clean_function(function, used),
        },
        StmtKind::Function { target, function } => StmtKind::Function {
            target: target.clone(),
            function: clean_function(function, used),
        },
        StmtKind::Local { names, values } => StmtKind::Local {
            names: names.clone(),
            values: values.clone(),
        },
        StmtKind::Assign { targets, values } => StmtKind::Assign {
            targets: targets.clone(),
            values: values.clone(),
        },
        StmtKind::Call(expr) => StmtKind::Call(expr.clone()),
        StmtKind::Return(values) => StmtKind::Return(values.clone()),
        StmtKind::Break => StmtKind::Break,
        StmtKind::Raw(text) => StmtKind::Raw(text.clone()),
    };
    let changed = !same_stmt_kind(&statement.kind, &kind);
    if changed {
        make_stmt(
            &statement.span,
            kind,
            statement.leading.clone(),
            statement.trailing.clone(),
        )
    } else {
        statement.clone()
    }
}

fn same_stmt_kind(left: &StmtKind, right: &StmtKind) -> bool {
    std::mem::discriminant(left) == std::mem::discriminant(right)
        && blocks_equal(left, right)
}

fn blocks_equal(left: &StmtKind, right: &StmtKind) -> bool {
    let left_blocks = collect_blocks(left);
    let right_blocks = collect_blocks(right);
    left_blocks.len() == right_blocks.len()
        && left_blocks.iter().zip(right_blocks.iter()).all(|(a, b)| {
            a.statements.len() == b.statements.len()
                && a.close_leading == b.close_leading
        })
}

fn collect_blocks(kind: &StmtKind) -> Vec<&Block> {
    match kind {
        StmtKind::If {
            branches,
            else_body,
        } => {
            let mut blocks = Vec::new();
            for branch in branches {
                blocks.push(&branch.body);
            }
            if let Some(body) = else_body {
                blocks.push(body);
            }
            blocks
        }
        StmtKind::Do(body)
        | StmtKind::While { body, .. }
        | StmtKind::Repeat { body, .. }
        | StmtKind::NumericFor { body, .. }
        | StmtKind::GenericFor { body, .. } => vec![body],
        StmtKind::LocalFunction { function }
        | StmtKind::Function { function, .. } => {
            vec![&function.body]
        }
        _ => Vec::new(),
    }
}

fn clean_block(block: &Block, used: &BTreeSet<String>) -> Block {
    let mut statements = Vec::new();
    let mut stopped = false;
    for statement in &block.statements {
        if stopped {
            break;
        }
        if matches!(statement.kind, StmtKind::Return(_)) {
            statements.push(statement.clone());
            stopped = true;
            continue;
        }
        if removable(statement, used) {
            continue;
        }
        statements.push(clean_stmt(statement, used));
    }
    Block {
        span: block.span.clone(),
        statements,
        close_leading: block.close_leading.clone(),
    }
}

fn clean_function(function: &Function, used: &BTreeSet<String>) -> Function {
    let body = clean_block(&function.body, used);
    if body == function.body {
        return function.clone();
    }
    Function {
        name: function.name.clone(),
        span: function.span.clone(),
        params: function.params.clone(),
        varargs: function.varargs,
        body,
        original: None,
    }
}

pub(crate) fn prune_callbacks(script: &Script) -> Script {
    let mut used = BTreeSet::new();
    for statement in &script.statements {
        let mut names = BTreeSet::new();
        stmt_names(statement, &mut names);
        if let StmtKind::Function { target, .. } = &statement.kind
            && let Some(name) = target.global_name()
        {
            let _ = names.remove(name);
        }
        used.extend(names);
    }
    let mut result = script.clone();
    result.statements = script
        .statements
        .iter()
        .filter(|statement| !prunable_callback(statement, &used))
        .cloned()
        .collect();
    result
}

fn prunable_callback(statement: &Stmt, used: &BTreeSet<String>) -> bool {
    match &statement.kind {
        StmtKind::Function { target, .. } => {
            let Some(name) = target.global_name() else {
                return false;
            };
            if name != "__strict_callback" || used.contains(name) {
                return false;
            }
            return true;
        }
        StmtKind::LocalFunction { function } => {
            let name = function_local_name(function);
            function.body.statements.is_empty()
                && !name.is_empty()
                && !used.contains(name)
        }
        StmtKind::Local { names, values } => {
            if names.len() != 1 || values.len() != 1 {
                return false;
            }
            let Some(name) = names.first() else {
                return false;
            };
            let Some(value) = values.first() else {
                return false;
            };
            !used.contains(name) && matches!(value.kind, ExprKind::Function(_))
        }
        _ => false,
    }
}

pub(crate) fn specialize_tables(script: &Script) -> Script {
    let folded = fold_is_array_calls(script);
    let hoisted = hoist_tables(&folded);
    let mut result = specialize_arrays(&hoisted);
    if !has_is_array_call_script(&result) {
        result.statements.retain(|statement| {
            !matches!(&statement.kind, StmtKind::LocalFunction { function }
                if function_local_name(function) == "is_array")
        });
    }
    result
}

fn collect_script_stmts<'a>(script: &'a Script, out: &mut Vec<&'a Stmt>) {
    for statement in &script.statements {
        collect_stmt_stmts(statement, out);
    }
}

fn collect_stmt_stmts<'a>(statement: &'a Stmt, out: &mut Vec<&'a Stmt>) {
    out.push(statement);
    match &statement.kind {
        StmtKind::LocalFunction { function }
        | StmtKind::Function { function, .. } => {
            collect_block_stmts(&function.body, out);
        }
        StmtKind::If {
            branches,
            else_body,
        } => {
            for branch in branches {
                collect_block_stmts(&branch.body, out);
            }
            if let Some(body) = else_body {
                collect_block_stmts(body, out);
            }
        }
        StmtKind::Do(body)
        | StmtKind::While { body, .. }
        | StmtKind::Repeat { body, .. }
        | StmtKind::NumericFor { body, .. }
        | StmtKind::GenericFor { body, .. } => collect_block_stmts(body, out),
        StmtKind::Local { values, .. } | StmtKind::Return(values) => {
            for value in values {
                collect_expr_functions(value, out);
            }
        }
        StmtKind::Assign { targets, values } => {
            for target in targets {
                collect_expr_functions(target, out);
            }
            for value in values {
                collect_expr_functions(value, out);
            }
        }
        StmtKind::Call(expr) => collect_expr_functions(expr, out),
        StmtKind::Break | StmtKind::Raw(_) => {}
    }
}

fn collect_block_stmts<'a>(block: &'a Block, out: &mut Vec<&'a Stmt>) {
    for statement in &block.statements {
        collect_stmt_stmts(statement, out);
    }
}

fn collect_expr_functions<'a>(expression: &'a Expr, out: &mut Vec<&'a Stmt>) {
    match &expression.kind {
        ExprKind::Function(function) => {
            collect_block_stmts(&function.body, out)
        }
        ExprKind::Paren(inner) | ExprKind::Unary { operand: inner, .. } => {
            collect_expr_functions(inner, out);
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            collect_expr_functions(lhs, out);
            collect_expr_functions(rhs, out);
        }
        ExprKind::Field { object, .. } => collect_expr_functions(object, out),
        ExprKind::Index { object, index } => {
            collect_expr_functions(object, out);
            collect_expr_functions(index, out);
        }
        ExprKind::Call { callee, args } => {
            collect_expr_functions(callee, out);
            for arg in args {
                collect_expr_functions(arg, out);
            }
        }
        ExprKind::MethodCall { object, args, .. } => {
            collect_expr_functions(object, out);
            for arg in args {
                collect_expr_functions(arg, out);
            }
        }
        ExprKind::Table(fields) => {
            for field in fields {
                match &field.kind {
                    TableFieldKind::Named(_, value)
                    | TableFieldKind::Positional(value) => {
                        collect_expr_functions(value, out);
                    }
                    TableFieldKind::Keyed(key, value) => {
                        collect_expr_functions(key, out);
                        collect_expr_functions(value, out);
                    }
                }
            }
        }
        ExprKind::Nil
        | ExprKind::Bool(_)
        | ExprKind::Integer(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Varargs
        | ExprKind::Var(_)
        | ExprKind::Raw(_) => {}
    }
}

fn fold_is_array_calls(script: &Script) -> Script {
    let proven = proven_array_names(script);
    let mut result = script.clone();
    result.statements = fold_is_array_statements(&result.statements, &proven);
    result
}

fn fold_is_array_statements(
    statements: &[Stmt],
    proven: &BTreeSet<String>,
) -> Vec<Stmt> {
    statements
        .iter()
        .map(|statement| fold_is_array_statement(statement, proven))
        .collect()
}

fn fold_is_array_statement(
    statement: &Stmt,
    proven: &BTreeSet<String>,
) -> Stmt {
    let kind = fold_is_array_stmt(statement, proven)
        .unwrap_or_else(|| statement.kind.clone());
    let mut changed = kind != statement.kind;
    let mut kind = kind;
    match &mut kind {
        StmtKind::LocalFunction { function }
        | StmtKind::Function { function, .. } => {
            let body = fold_is_array_block(&function.body, proven);
            changed |= body != function.body;
            function.body = body;
            if changed {
                function.original = None;
            }
        }
        StmtKind::If {
            branches,
            else_body,
        } => {
            for branch in branches {
                let body = fold_is_array_block(&branch.body, proven);
                changed |= body != branch.body;
                branch.body = body;
            }
            if let Some(body) = else_body {
                let folded = fold_is_array_block(body, proven);
                changed |= &folded != body;
                *body = folded;
            }
        }
        StmtKind::Do(body)
        | StmtKind::While { body, .. }
        | StmtKind::Repeat { body, .. }
        | StmtKind::NumericFor { body, .. }
        | StmtKind::GenericFor { body, .. } => {
            let folded = fold_is_array_block(body, proven);
            changed |= &folded != body;
            *body = folded;
        }
        _ => {}
    }
    if changed {
        Stmt {
            span: statement.span.clone(),
            kind,
            leading: statement.leading.clone(),
            trailing: statement.trailing.clone(),
            original: None,
        }
    } else {
        statement.clone()
    }
}

fn fold_is_array_block(block: &Block, proven: &BTreeSet<String>) -> Block {
    Block {
        span: block.span.clone(),
        statements: fold_is_array_statements(&block.statements, proven),
        close_leading: block.close_leading.clone(),
    }
}

fn proven_array_names(script: &Script) -> BTreeSet<String> {
    let mut statements = Vec::new();
    collect_script_stmts(script, &mut statements);
    let mut candidates = BTreeSet::new();
    for statement in &statements {
        if let StmtKind::Local { names, values } = &statement.kind
            && let (Some(name), Some(value)) = (names.first(), values.first())
            && values.len() == 1
            && is_marked_array(value)
        {
            let _ = candidates.insert(name.clone());
        }
    }
    let mut proven = BTreeSet::new();
    for name in candidates {
        let declarations: Vec<_> = statements
            .iter()
            .filter(|statement| {
                matches!(&statement.kind, StmtKind::Local { names, .. } if names.contains(&name))
            })
            .collect();
        if declarations.is_empty()
            || declarations.iter().any(|statement| {
                !matches!(
                    &statement.kind,
                    StmtKind::Local { values, .. }
                        if values.len() == 1
                            && values.first().is_some_and(is_marked_array)
                )
            })
            || statements
                .iter()
                .any(|statement| has_assignment_stmt(statement, &name))
        {
            continue;
        }
        let occurrences: usize = statements
            .iter()
            .map(|statement| count_var_stmt(statement, &name))
            .sum();
        let arguments: usize = statements
            .iter()
            .map(|statement| count_is_array_arg_stmt(statement, &name))
            .sum();
        if occurrences == arguments {
            let _ = proven.insert(name);
        }
    }
    proven
}

fn fold_is_array_stmt(
    statement: &Stmt,
    proven: &BTreeSet<String>,
) -> Option<StmtKind> {
    match &statement.kind {
        StmtKind::Local { names, values } => {
            let (folded, changed) = fold_is_array_list(values, proven);
            changed.then_some(StmtKind::Local {
                names: names.clone(),
                values: folded,
            })
        }
        StmtKind::Assign { targets, values } => {
            let (folded_targets, targets_changed) =
                fold_is_array_list(targets, proven);
            let (folded_values, values_changed) =
                fold_is_array_list(values, proven);
            (targets_changed || values_changed).then_some(StmtKind::Assign {
                targets: folded_targets,
                values: folded_values,
            })
        }
        StmtKind::Return(values) => {
            let (folded, changed) = fold_is_array_list(values, proven);
            changed.then_some(StmtKind::Return(folded))
        }
        StmtKind::Call(expr) => fold_is_array_expr(expr, proven)
            .map(|folded| StmtKind::Call(folded)),
        StmtKind::NumericFor {
            lead,
            name,
            start,
            limit,
            step,
            tail,
            body,
        } => {
            let folded_start = fold_is_array_expr(start, proven);
            let folded_limit = fold_is_array_expr(limit, proven);
            let folded_step = step
                .as_ref()
                .and_then(|expr| fold_is_array_expr(expr, proven));
            let changed = folded_start.is_some()
                || folded_limit.is_some()
                || folded_step.is_some();
            changed.then_some(StmtKind::NumericFor {
                lead: lead.clone(),
                name: name.clone(),
                start: folded_start.unwrap_or_else(|| start.clone()),
                limit: folded_limit.unwrap_or_else(|| limit.clone()),
                step: folded_step.or_else(|| step.clone()),
                tail: tail.clone(),
                body: body.clone(),
            })
        }
        StmtKind::GenericFor {
            lead,
            names,
            iterator,
            tail,
            body,
        } => fold_is_array_expr(iterator, proven).map(|folded| {
            StmtKind::GenericFor {
                lead: lead.clone(),
                names: names.clone(),
                iterator: folded,
                tail: tail.clone(),
                body: body.clone(),
            }
        }),
        StmtKind::If {
            branches,
            else_body,
        } => {
            let mut new_branches = Vec::new();
            let mut changed = false;
            for branch in branches {
                let folded = fold_is_array_expr(&branch.condition, proven);
                changed |= folded.is_some();
                new_branches.push(Branch {
                    span: branch.span.clone(),
                    keyword: branch.keyword.clone(),
                    lead: branch.lead.clone(),
                    condition: folded
                        .unwrap_or_else(|| branch.condition.clone()),
                    tail: branch.tail.clone(),
                    body: branch.body.clone(),
                });
            }
            changed.then_some(StmtKind::If {
                branches: new_branches,
                else_body: else_body.clone(),
            })
        }
        StmtKind::While {
            lead,
            condition,
            tail,
            body,
        } => fold_is_array_expr(condition, proven).map(|folded| {
            StmtKind::While {
                lead: lead.clone(),
                condition: folded,
                tail: tail.clone(),
                body: body.clone(),
            }
        }),
        StmtKind::Repeat { body, condition } => {
            fold_is_array_expr(condition, proven).map(|folded| {
                StmtKind::Repeat {
                    body: body.clone(),
                    condition: folded,
                }
            })
        }
        StmtKind::Break
        | StmtKind::Do(_)
        | StmtKind::LocalFunction { .. }
        | StmtKind::Function { .. }
        | StmtKind::Raw(_) => None,
    }
}

fn fold_is_array_list(
    expressions: &[Expr],
    proven: &BTreeSet<String>,
) -> (Vec<Expr>, bool) {
    let mut folded = Vec::new();
    let mut changed = false;
    for expression in expressions {
        let result = fold_is_array_expr(expression, proven);
        changed |= result.is_some();
        folded.push(result.unwrap_or_else(|| expression.clone()));
    }
    (folded, changed)
}

fn fold_is_array_expr(
    expression: &Expr,
    proven: &BTreeSet<String>,
) -> Option<Expr> {
    if let ExprKind::Call { callee, args } = &expression.kind {
        if callee.global_name() == Some("is_array") {
            if let Some(name) = args.first().and_then(Expr::global_name) {
                if proven.contains(name) {
                    return Some(make_expr(
                        &expression.span,
                        ExprKind::Bool(true),
                    ));
                }
            }
        }
    }
    if let Some(name) = is_array_argument(expression) {
        if proven.contains(name) {
            return Some(make_expr(&expression.span, ExprKind::Bool(true)));
        }
    }
    match &expression.kind {
        ExprKind::Binary { lhs, op, rhs } => {
            let folded_lhs = fold_is_array_expr(lhs, proven);
            let folded_rhs = fold_is_array_expr(rhs, proven);
            if folded_lhs.is_none() && folded_rhs.is_none() {
                return None;
            }
            Some(make_expr(
                &expression.span,
                ExprKind::Binary {
                    lhs: Box::new(
                        folded_lhs.unwrap_or_else(|| lhs.as_ref().clone()),
                    ),
                    op: op.clone(),
                    rhs: Box::new(
                        folded_rhs.unwrap_or_else(|| rhs.as_ref().clone()),
                    ),
                },
            ))
        }
        ExprKind::Paren(inner) => {
            fold_is_array_expr(inner, proven).map(|folded| {
                make_expr(&expression.span, ExprKind::Paren(Box::new(folded)))
            })
        }
        ExprKind::Call { callee, args } => {
            let folded_args = args
                .iter()
                .map(|arg| {
                    fold_is_array_expr(arg, proven)
                        .unwrap_or_else(|| arg.clone())
                })
                .collect::<Vec<_>>();
            if folded_args.iter().zip(args.iter()).all(|(new, old)| {
                new.original.is_some() || new.kind != old.kind
            }) {
                return Some(make_expr(
                    &expression.span,
                    ExprKind::Call {
                        callee: callee.clone(),
                        args: folded_args,
                    },
                ));
            }
            None
        }
        _ => None,
    }
}

fn is_array_argument(expression: &Expr) -> Option<&str> {
    let ExprKind::Var(name) = &expression.kind else {
        return None;
    };
    Some(name)
}

fn count_var_expr(expression: &Expr, target: &str) -> usize {
    let nested = match &expression.kind {
        ExprKind::Var(name) => return usize::from(name == target),
        ExprKind::Paren(inner) => count_var_expr(inner, target),
        ExprKind::Field { object, .. } => count_var_expr(object, target),
        ExprKind::Index { object, index } => count_var_expr(object, target)
            .saturating_add(count_var_expr(index, target)),
        ExprKind::Unary { operand, .. } => count_var_expr(operand, target),
        ExprKind::Binary { lhs, rhs, .. } => count_var_expr(lhs, target)
            .saturating_add(count_var_expr(rhs, target)),
        ExprKind::Call { callee, args } => {
            let mut count = count_var_expr(callee, target);
            for arg in args {
                count = count.saturating_add(count_var_expr(arg, target));
            }
            count
        }
        ExprKind::MethodCall { object, args, .. } => {
            let mut count = count_var_expr(object, target);
            for arg in args {
                count = count.saturating_add(count_var_expr(arg, target));
            }
            count
        }
        ExprKind::Table(fields) => {
            let mut count = 0usize;
            for field in fields {
                match &field.kind {
                    TableFieldKind::Named(_, value)
                    | TableFieldKind::Positional(value) => {
                        count =
                            count.saturating_add(count_var_expr(value, target));
                    }
                    TableFieldKind::Keyed(key, value) => {
                        count =
                            count.saturating_add(count_var_expr(key, target));
                        count =
                            count.saturating_add(count_var_expr(value, target));
                    }
                }
            }
            count
        }
        ExprKind::Function(function) => count_var_block(&function.body, target),
        ExprKind::Nil
        | ExprKind::Bool(_)
        | ExprKind::Integer(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Varargs
        | ExprKind::Raw(_) => 0,
    };
    nested
}

fn count_var_block(block: &Block, target: &str) -> usize {
    block
        .statements
        .iter()
        .map(|statement| count_var_stmt(statement, target))
        .sum()
}

fn count_var_stmt(statement: &Stmt, target: &str) -> usize {
    match &statement.kind {
        StmtKind::Local { values, .. } | StmtKind::Return(values) => {
            values.iter().map(|expr| count_var_expr(expr, target)).sum()
        }
        StmtKind::Assign { targets: _, values } => {
            values.iter().map(|expr| count_var_expr(expr, target)).sum()
        }
        StmtKind::Call(expr) => count_var_expr(expr, target),
        StmtKind::If {
            branches,
            else_body,
        } => {
            let mut count = 0usize;
            for branch in branches {
                count = count
                    .saturating_add(count_var_expr(&branch.condition, target));
                count =
                    count.saturating_add(count_var_block(&branch.body, target));
            }
            if let Some(body) = else_body {
                count = count.saturating_add(count_var_block(body, target));
            }
            count
        }
        StmtKind::Do(body) => count_var_block(body, target),
        StmtKind::While {
            condition, body, ..
        }
        | StmtKind::Repeat {
            condition, body, ..
        } => count_var_expr(condition, target)
            .saturating_add(count_var_block(body, target)),
        StmtKind::NumericFor {
            start,
            limit,
            step,
            body,
            ..
        } => {
            let mut count = count_var_expr(start, target)
                .saturating_add(count_var_expr(limit, target));
            if let Some(step) = step {
                count = count.saturating_add(count_var_expr(step, target));
            }
            count.saturating_add(count_var_block(body, target))
        }
        StmtKind::GenericFor { iterator, body, .. } => {
            count_var_expr(iterator, target)
                .saturating_add(count_var_block(body, target))
        }
        StmtKind::LocalFunction { function }
        | StmtKind::Function { function, .. } => {
            count_var_block(&function.body, target)
        }
        StmtKind::Break | StmtKind::Raw(_) => 0,
    }
}

fn count_is_array_arg_expr(expression: &Expr, target: &str) -> usize {
    let nested = if let ExprKind::Call { callee, args } = &expression.kind {
        let direct = callee.global_name() == Some("is_array")
            && args
                .first()
                .and_then(Expr::global_name)
                .is_some_and(|name| name == target);
        let mut count = usize::from(direct);
        for arg in args {
            count = count.saturating_add(count_is_array_arg_expr(arg, target));
        }
        count
    } else {
        match &expression.kind {
            ExprKind::Call { callee, args } => {
                let mut count = count_is_array_arg_expr(callee, target);
                for arg in args {
                    count = count
                        .saturating_add(count_is_array_arg_expr(arg, target));
                }
                count
            }
            ExprKind::Paren(inner) => count_is_array_arg_expr(inner, target),
            ExprKind::Field { object, .. } => {
                count_is_array_arg_expr(object, target)
            }
            ExprKind::Index { object, index } => {
                count_is_array_arg_expr(object, target)
                    .saturating_add(count_is_array_arg_expr(index, target))
            }
            ExprKind::Unary { operand, .. } => {
                count_is_array_arg_expr(operand, target)
            }
            ExprKind::Binary { lhs, rhs, .. } => {
                count_is_array_arg_expr(lhs, target)
                    .saturating_add(count_is_array_arg_expr(rhs, target))
            }
            ExprKind::MethodCall { object, args, .. } => {
                let mut count = count_is_array_arg_expr(object, target);
                for arg in args {
                    count = count
                        .saturating_add(count_is_array_arg_expr(arg, target));
                }
                count
            }
            ExprKind::Table(fields) => {
                let mut count = 0usize;
                for field in fields {
                    match &field.kind {
                        TableFieldKind::Named(_, value)
                        | TableFieldKind::Positional(value) => {
                            count = count.saturating_add(
                                count_is_array_arg_expr(value, target),
                            );
                        }
                        TableFieldKind::Keyed(key, value) => {
                            count = count.saturating_add(
                                count_is_array_arg_expr(key, target),
                            );
                            count = count.saturating_add(
                                count_is_array_arg_expr(value, target),
                            );
                        }
                    }
                }
                count
            }
            ExprKind::Function(function) => {
                count_is_array_arg_block(&function.body, target)
            }
            ExprKind::Nil
            | ExprKind::Bool(_)
            | ExprKind::Integer(_)
            | ExprKind::Float(_)
            | ExprKind::String(_)
            | ExprKind::Varargs
            | ExprKind::Var(_)
            | ExprKind::Raw(_) => 0,
        }
    };
    nested
}

fn count_is_array_arg_block(block: &Block, target: &str) -> usize {
    block
        .statements
        .iter()
        .map(|statement| count_is_array_arg_stmt(statement, target))
        .sum()
}

fn count_is_array_arg_stmt(statement: &Stmt, target: &str) -> usize {
    match &statement.kind {
        StmtKind::Local { values, .. } | StmtKind::Return(values) => values
            .iter()
            .map(|expr| count_is_array_arg_expr(expr, target))
            .sum(),
        StmtKind::Assign { targets: _, values } => values
            .iter()
            .map(|expr| count_is_array_arg_expr(expr, target))
            .sum(),
        StmtKind::Call(expr) => count_is_array_arg_expr(expr, target),
        StmtKind::If {
            branches,
            else_body,
        } => {
            let mut count = 0usize;
            for branch in branches {
                count = count.saturating_add(count_is_array_arg_expr(
                    &branch.condition,
                    target,
                ));
                count = count.saturating_add(count_is_array_arg_block(
                    &branch.body,
                    target,
                ));
            }
            if let Some(body) = else_body {
                count = count
                    .saturating_add(count_is_array_arg_block(body, target));
            }
            count
        }
        StmtKind::Do(body) => count_is_array_arg_block(body, target),
        StmtKind::While {
            condition, body, ..
        }
        | StmtKind::Repeat {
            condition, body, ..
        } => count_is_array_arg_expr(condition, target)
            .saturating_add(count_is_array_arg_block(body, target)),
        StmtKind::NumericFor {
            start,
            limit,
            step,
            body,
            ..
        } => {
            let mut count = count_is_array_arg_expr(start, target)
                .saturating_add(count_is_array_arg_expr(limit, target));
            if let Some(step) = step {
                count =
                    count.saturating_add(count_is_array_arg_expr(step, target));
            }
            count.saturating_add(count_is_array_arg_block(body, target))
        }
        StmtKind::GenericFor { iterator, body, .. } => {
            count_is_array_arg_expr(iterator, target)
                .saturating_add(count_is_array_arg_block(body, target))
        }
        StmtKind::LocalFunction { function }
        | StmtKind::Function { function, .. } => {
            count_is_array_arg_block(&function.body, target)
        }
        StmtKind::Break | StmtKind::Raw(_) => 0,
    }
}

fn has_assignment_stmt(statement: &Stmt, target: &str) -> bool {
    match &statement.kind {
        StmtKind::Assign { targets, .. } => targets.iter().any(
            |expr| matches!(&expr.kind, ExprKind::Var(name) if name == target),
        ),
        StmtKind::If {
            branches,
            else_body,
        } => {
            branches
                .iter()
                .any(|branch| has_assignment_block(&branch.body, target))
                || else_body
                    .as_ref()
                    .is_some_and(|body| has_assignment_block(body, target))
        }
        StmtKind::Do(body)
        | StmtKind::While { body, .. }
        | StmtKind::Repeat { body, .. }
        | StmtKind::NumericFor { body, .. }
        | StmtKind::GenericFor { body, .. } => {
            has_assignment_block(body, target)
        }
        StmtKind::LocalFunction { function }
        | StmtKind::Function { function, .. } => {
            has_assignment_block(&function.body, target)
        }
        StmtKind::Local { .. }
        | StmtKind::Return(_)
        | StmtKind::Call(_)
        | StmtKind::Break
        | StmtKind::Raw(_) => false,
    }
}

fn has_assignment_block(block: &Block, target: &str) -> bool {
    block
        .statements
        .iter()
        .any(|statement| has_assignment_stmt(statement, target))
}

fn is_marked_array(expression: &Expr) -> bool {
    let ExprKind::Call { callee, args } = &expression.kind else {
        return false;
    };
    if callee.global_name() != Some("setmetatable") || args.len() != 2 {
        return false;
    }
    let Some(array) = args.get(..1).and_then(|slice| slice.first()) else {
        return false;
    };
    let Some(metatable) = args.get(1..2).and_then(|slice| slice.first()) else {
        return false;
    };
    is_empty_or_pure_array_table(array) && is_array_marker_table(metatable)
}

fn is_array_marker_table(expression: &Expr) -> bool {
    let ExprKind::Table(fields) = &expression.kind else {
        return false;
    };
    fields.len() == 1
        && fields.iter().all(|field| match &field.kind {
            TableFieldKind::Named(name, value) => {
                name == "__noctalia_array"
                    && matches!(value.kind, ExprKind::Bool(true))
            }
            _ => false,
        })
}

fn is_pure_array_table(expression: &Expr) -> bool {
    let ExprKind::Table(fields) = &expression.kind else {
        return false;
    };
    !fields.is_empty()
        && fields.iter().all(|field| match &field.kind {
            TableFieldKind::Positional(value) => {
                value.is_constant() && !is_nil(value)
            }
            _ => false,
        })
}

fn is_empty_or_pure_array_table(expression: &Expr) -> bool {
    let ExprKind::Table(fields) = &expression.kind else {
        return false;
    };
    fields.iter().all(|field| match &field.kind {
        TableFieldKind::Positional(value) => {
            value.is_constant() && !is_nil(value)
        }
        _ => false,
    })
}

fn is_nil(expression: &Expr) -> bool {
    matches!(expression.kind, ExprKind::Nil)
}

fn strip_expr(expression: &Expr) -> Expr {
    let kind = match &expression.kind {
        ExprKind::Paren(inner) => ExprKind::Paren(Box::new(strip_expr(inner))),
        ExprKind::Field {
            object,
            field,
            separator,
        } => ExprKind::Field {
            object: Box::new(strip_expr(object)),
            field: field.clone(),
            separator: separator.clone(),
        },
        ExprKind::Index { object, index } => ExprKind::Index {
            object: Box::new(strip_expr(object)),
            index: Box::new(strip_expr(index)),
        },
        ExprKind::Unary { op, operand } => ExprKind::Unary {
            op: op.clone(),
            operand: Box::new(strip_expr(operand)),
        },
        ExprKind::Binary { lhs, op, rhs } => ExprKind::Binary {
            lhs: Box::new(strip_expr(lhs)),
            op: op.clone(),
            rhs: Box::new(strip_expr(rhs)),
        },
        ExprKind::Call { callee, args } => ExprKind::Call {
            callee: Box::new(strip_expr(callee)),
            args: args.iter().map(strip_expr).collect(),
        },
        ExprKind::MethodCall {
            object,
            method,
            args,
        } => ExprKind::MethodCall {
            object: Box::new(strip_expr(object)),
            method: method.clone(),
            args: args.iter().map(strip_expr).collect(),
        },
        ExprKind::Table(fields) => ExprKind::Table(
            fields
                .iter()
                .map(|field| TableField {
                    span: field.span.clone(),
                    kind: match &field.kind {
                        TableFieldKind::Named(name, value) => {
                            TableFieldKind::Named(
                                name.clone(),
                                strip_expr(value),
                            )
                        }
                        TableFieldKind::Keyed(key, value) => {
                            TableFieldKind::Keyed(
                                strip_expr(key),
                                strip_expr(value),
                            )
                        }
                        TableFieldKind::Positional(value) => {
                            TableFieldKind::Positional(strip_expr(value))
                        }
                    },
                    leading: String::new(),
                    separator: field.separator.clone(),
                    original: None,
                })
                .collect(),
        ),
        ExprKind::Function(function) => ExprKind::Function(Function {
            name: function.name.clone(),
            span: function.span.clone(),
            params: function.params.clone(),
            varargs: function.varargs,
            body: function.body.clone(),
            original: None,
        }),
        ExprKind::Nil => ExprKind::Nil,
        ExprKind::Bool(value) => ExprKind::Bool(*value),
        ExprKind::Integer(value) => ExprKind::Integer(*value),
        ExprKind::Float(value) => ExprKind::Float(value.clone()),
        ExprKind::String(value) => ExprKind::String(value.clone()),
        ExprKind::Varargs => ExprKind::Varargs,
        ExprKind::Var(name) => ExprKind::Var(name.clone()),
        ExprKind::Raw(text) => ExprKind::Raw(text.clone()),
    };
    make_expr(&expression.span, kind)
}

fn table_key(expression: &Expr) -> Option<String> {
    if !expression.is_constant() || !has_table(expression) {
        return None;
    }
    if contains_nil(expression) {
        return None;
    }
    Some(expr_key(&strip_expr(expression)))
}

fn expr_key(expression: &Expr) -> String {
    match &expression.kind {
        ExprKind::Nil => "nil".to_owned(),
        ExprKind::Bool(value) => format!("bool:{value}"),
        ExprKind::Integer(value) => format!("int:{value}"),
        ExprKind::Float(value) => format!("float:{value}"),
        ExprKind::String(value) => format!("string:{value:?}"),
        ExprKind::Varargs => "varargs".to_owned(),
        ExprKind::Var(name) => format!("var:{name}"),
        ExprKind::Field {
            object,
            field,
            separator,
        } => format!("field:{}{}{}", expr_key(object), field, separator),
        ExprKind::Index { object, index } => {
            format!("index:{}:{}", expr_key(object), expr_key(index))
        }
        ExprKind::Unary { op, operand } => {
            format!("unary:{op}:{}", expr_key(operand))
        }
        ExprKind::Binary { lhs, op, rhs } => {
            format!("binary:{}:{op}:{}", expr_key(lhs), expr_key(rhs))
        }
        ExprKind::Paren(inner) => format!("paren:{}", expr_key(inner)),
        ExprKind::Call { callee, args } => format!(
            "call:{}:{}",
            expr_key(callee),
            args.iter().map(expr_key).collect::<Vec<_>>().join("|")
        ),
        ExprKind::MethodCall {
            object,
            method,
            args,
        } => format!(
            "method:{}:{method}:{}",
            expr_key(object),
            args.iter().map(expr_key).collect::<Vec<_>>().join("|")
        ),
        ExprKind::Table(fields) => format!(
            "table:{}",
            fields
                .iter()
                .map(table_field_key)
                .collect::<Vec<_>>()
                .join("|")
        ),
        ExprKind::Function(_) => "function".to_owned(),
        ExprKind::Raw(text) => format!("raw:{text:?}"),
    }
}

fn table_field_key(field: &TableField) -> String {
    match &field.kind {
        TableFieldKind::Named(name, value) => {
            format!("named:{name}:{}", expr_key(value))
        }
        TableFieldKind::Keyed(key, value) => {
            format!("keyed:{}:{}", expr_key(key), expr_key(value))
        }
        TableFieldKind::Positional(value) => {
            format!("positional:{}", expr_key(value))
        }
    }
}

fn has_table(expression: &Expr) -> bool {
    match &expression.kind {
        ExprKind::Table(_) => true,
        ExprKind::Paren(inner) => has_table(inner),
        ExprKind::Binary { lhs, rhs, .. } => has_table(lhs) || has_table(rhs),
        ExprKind::Unary { operand, .. } => has_table(operand),
        _ => false,
    }
}

fn contains_nil(expression: &Expr) -> bool {
    match &expression.kind {
        ExprKind::Nil => true,
        ExprKind::Paren(inner) => contains_nil(inner),
        ExprKind::Binary { lhs, rhs, .. } => {
            contains_nil(lhs) || contains_nil(rhs)
        }
        ExprKind::Unary { operand, .. } => contains_nil(operand),
        ExprKind::Table(fields) => {
            fields.iter().any(|field| match &field.kind {
                TableFieldKind::Named(_, value)
                | TableFieldKind::Positional(value) => contains_nil(value),
                TableFieldKind::Keyed(key, value) => {
                    contains_nil(key) || contains_nil(value)
                }
            })
        }
        ExprKind::Call { args, .. } => args.iter().any(contains_nil),
        ExprKind::MethodCall { args, .. } => args.iter().any(contains_nil),
        _ => false,
    }
}

fn collect_table_keys_expr(
    expression: &Expr,
    counts: &mut BTreeMap<String, usize>,
) {
    if let Some(key) = table_key(expression) {
        let count = counts.entry(key).or_insert(0usize);
        *count = count.saturating_add(1);
    }
    match &expression.kind {
        ExprKind::Paren(inner) => collect_table_keys_expr(inner, counts),
        ExprKind::Field { object, .. } => {
            collect_table_keys_expr(object, counts)
        }
        ExprKind::Index { object, index } => {
            collect_table_keys_expr(object, counts);
            collect_table_keys_expr(index, counts);
        }
        ExprKind::Unary { operand, .. } => {
            collect_table_keys_expr(operand, counts)
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            collect_table_keys_expr(lhs, counts);
            collect_table_keys_expr(rhs, counts);
        }
        ExprKind::Call { callee, args } => {
            collect_table_keys_expr(callee, counts);
            for arg in args {
                collect_table_keys_expr(arg, counts);
            }
        }
        ExprKind::MethodCall { object, args, .. } => {
            collect_table_keys_expr(object, counts);
            for arg in args {
                collect_table_keys_expr(arg, counts);
            }
        }
        ExprKind::Table(fields) => {
            for field in fields {
                match &field.kind {
                    TableFieldKind::Named(_, value)
                    | TableFieldKind::Positional(value) => {
                        collect_table_keys_expr(value, counts);
                    }
                    TableFieldKind::Keyed(key, value) => {
                        collect_table_keys_expr(key, counts);
                        collect_table_keys_expr(value, counts);
                    }
                }
            }
        }
        ExprKind::Function(_) => {}
        ExprKind::Nil
        | ExprKind::Bool(_)
        | ExprKind::Integer(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Varargs
        | ExprKind::Var(_)
        | ExprKind::Raw(_) => {}
    }
}

fn collect_table_keys_stmt(
    statement: &Stmt,
    counts: &mut BTreeMap<String, usize>,
) {
    match &statement.kind {
        StmtKind::Local { values, .. } | StmtKind::Return(values) => {
            for value in values {
                collect_table_keys_expr(value, counts);
            }
        }
        StmtKind::Assign { targets, values } => {
            for target in targets {
                collect_table_keys_expr(target, counts);
            }
            for value in values {
                collect_table_keys_expr(value, counts);
            }
        }
        StmtKind::Call(expr) => collect_table_keys_expr(expr, counts),
        StmtKind::While { condition, .. }
        | StmtKind::Repeat { condition, .. } => {
            collect_table_keys_expr(condition, counts);
        }
        StmtKind::NumericFor {
            start, limit, step, ..
        } => {
            collect_table_keys_expr(start, counts);
            collect_table_keys_expr(limit, counts);
            if let Some(step) = step {
                collect_table_keys_expr(step, counts);
            }
        }
        StmtKind::GenericFor { iterator, .. } => {
            collect_table_keys_expr(iterator, counts)
        }
        StmtKind::If { branches, .. } => {
            for branch in branches {
                collect_table_keys_expr(&branch.condition, counts);
            }
        }
        StmtKind::LocalFunction { .. }
        | StmtKind::Function { .. }
        | StmtKind::Break
        | StmtKind::Do(_)
        | StmtKind::Raw(_) => {}
    }
}

fn hoist_tables(script: &Script) -> Script {
    let mut counts = BTreeMap::new();
    for statement in &script.statements {
        collect_table_keys_stmt(statement, &mut counts);
    }
    let mut used = BTreeSet::new();
    for statement in &script.statements {
        stmt_names(statement, &mut used);
    }
    let mut names = BTreeMap::new();
    let mut index = 0usize;
    for (key, count) in &counts {
        if *count < 2 {
            continue;
        }
        let name = format!("__noctalia_table_{}", index);
        index = index.saturating_add(1);
        if used.contains(&name) {
            continue;
        }
        let _ = names.insert(key.clone(), name);
    }
    if names.is_empty() {
        return script.clone();
    }
    let mut definitions = Vec::new();
    for (key, name) in &names {
        let mut stored = None;
        for statement in &script.statements {
            if let Some(expression) = first_table_with_key(statement, key) {
                stored = Some(expression.clone());
                break;
            }
        }
        if let Some(expression) = stored {
            definitions.push(Stmt {
                span: expression.span.clone(),
                kind: StmtKind::Local {
                    names: vec![name.clone()],
                    values: vec![expression],
                },
                leading: String::new(),
                trailing: String::new(),
                original: None,
            });
        }
    }
    let mut result = script.clone();
    let first_leading = script
        .statements
        .first()
        .map_or_else(String::new, |statement| statement.leading.clone());
    if let Some(first) = result.statements.first_mut() {
        first.leading.clear();
    }
    for (index, definition) in definitions.iter_mut().enumerate() {
        definition.leading = if index == 0 {
            first_leading.clone()
        } else {
            "\n".to_owned()
        };
    }
    for statement in &mut result.statements {
        replace_tables_stmt(statement, &names);
    }
    definitions.extend(result.statements);
    result.statements = definitions;
    result
}

fn first_table_with_key<'a>(
    statement: &'a Stmt,
    key: &str,
) -> Option<&'a Expr> {
    let expressions: Vec<&Expr> = match &statement.kind {
        StmtKind::Local { values, .. }
        | StmtKind::Return(values)
        | StmtKind::Assign { values, .. } => values.iter().collect(),
        StmtKind::Call(Expr {
            kind: ExprKind::Call { callee, args },
            ..
        }) => std::iter::once(callee.as_ref())
            .chain(args.iter())
            .collect(),
        StmtKind::Call(expr) => vec![expr],
        _ => Vec::new(),
    };
    expressions
        .into_iter()
        .find(|expr| table_key(expr).as_deref() == Some(key))
}

fn replace_tables_stmt(statement: &mut Stmt, names: &BTreeMap<String, String>) {
    let kind = match &statement.kind {
        StmtKind::Local {
            names: bound,
            values,
        } => {
            let folded = replace_tables_list(values, names);
            StmtKind::Local {
                names: bound.clone(),
                values: folded,
            }
        }
        StmtKind::Return(values) => {
            StmtKind::Return(replace_tables_list(values, names))
        }
        StmtKind::Assign { targets, values } => StmtKind::Assign {
            targets: replace_tables_list(targets, names),
            values: replace_tables_list(values, names),
        },
        StmtKind::Call(expr) => {
            StmtKind::Call(replace_tables_expr(expr, names))
        }
        StmtKind::While {
            lead,
            condition,
            tail,
            body,
        } => StmtKind::While {
            lead: lead.clone(),
            condition: replace_tables_expr(condition, names),
            tail: tail.clone(),
            body: body.clone(),
        },
        StmtKind::Repeat { body, condition } => StmtKind::Repeat {
            body: body.clone(),
            condition: replace_tables_expr(condition, names),
        },
        StmtKind::NumericFor {
            lead,
            name,
            start,
            limit,
            step,
            tail,
            body,
        } => StmtKind::NumericFor {
            lead: lead.clone(),
            name: name.clone(),
            start: replace_tables_expr(start, names),
            limit: replace_tables_expr(limit, names),
            step: step.as_ref().map(|expr| replace_tables_expr(expr, names)),
            tail: tail.clone(),
            body: body.clone(),
        },
        StmtKind::GenericFor {
            lead,
            names: bound,
            iterator,
            tail,
            body,
        } => StmtKind::GenericFor {
            lead: lead.clone(),
            names: bound.clone(),
            iterator: replace_tables_expr(iterator, names),
            tail: tail.clone(),
            body: body.clone(),
        },
        StmtKind::If {
            branches,
            else_body,
        } => StmtKind::If {
            branches: branches
                .iter()
                .map(|branch| Branch {
                    span: branch.span.clone(),
                    keyword: branch.keyword.clone(),
                    lead: branch.lead.clone(),
                    condition: replace_tables_expr(&branch.condition, names),
                    tail: branch.tail.clone(),
                    body: branch.body.clone(),
                })
                .collect(),
            else_body: else_body.clone(),
        },
        StmtKind::Break
        | StmtKind::Do(_)
        | StmtKind::LocalFunction { .. }
        | StmtKind::Function { .. }
        | StmtKind::Raw(_) => return,
    };
    statement.kind = kind;
    statement.original = None;
}

fn replace_tables_list(
    expressions: &[Expr],
    names: &BTreeMap<String, String>,
) -> Vec<Expr> {
    expressions
        .iter()
        .map(|expr| replace_tables_expr(expr, names))
        .collect()
}

fn replace_tables_expr(
    expression: &Expr,
    names: &BTreeMap<String, String>,
) -> Expr {
    if let Some(key) = table_key(expression) {
        if let Some(name) = names.get(&key) {
            return make_expr(&expression.span, ExprKind::Var(name.clone()));
        }
    }
    let kind = match &expression.kind {
        ExprKind::Paren(inner) => {
            ExprKind::Paren(Box::new(replace_tables_expr(inner, names)))
        }
        ExprKind::Field {
            object,
            field,
            separator,
        } => ExprKind::Field {
            object: Box::new(replace_tables_expr(object, names)),
            field: field.clone(),
            separator: separator.clone(),
        },
        ExprKind::Index { object, index } => ExprKind::Index {
            object: Box::new(replace_tables_expr(object, names)),
            index: Box::new(replace_tables_expr(index, names)),
        },
        ExprKind::Unary { op, operand } => ExprKind::Unary {
            op: op.clone(),
            operand: Box::new(replace_tables_expr(operand, names)),
        },
        ExprKind::Binary { lhs, op, rhs } => ExprKind::Binary {
            lhs: Box::new(replace_tables_expr(lhs, names)),
            op: op.clone(),
            rhs: Box::new(replace_tables_expr(rhs, names)),
        },
        ExprKind::Call { callee, args } => ExprKind::Call {
            callee: Box::new(replace_tables_expr(callee, names)),
            args: args
                .iter()
                .map(|arg| replace_tables_expr(arg, names))
                .collect(),
        },
        ExprKind::MethodCall {
            object,
            method,
            args,
        } => ExprKind::MethodCall {
            object: Box::new(replace_tables_expr(object, names)),
            method: method.clone(),
            args: args
                .iter()
                .map(|arg| replace_tables_expr(arg, names))
                .collect(),
        },
        ExprKind::Table(fields) => ExprKind::Table(
            fields
                .iter()
                .map(|field| TableField {
                    span: field.span.clone(),
                    kind: match &field.kind {
                        TableFieldKind::Named(name, value) => {
                            TableFieldKind::Named(
                                name.clone(),
                                replace_tables_expr(value, names),
                            )
                        }
                        TableFieldKind::Keyed(key, value) => {
                            TableFieldKind::Keyed(
                                replace_tables_expr(key, names),
                                replace_tables_expr(value, names),
                            )
                        }
                        TableFieldKind::Positional(value) => {
                            TableFieldKind::Positional(replace_tables_expr(
                                value, names,
                            ))
                        }
                    },
                    leading: field.leading.clone(),
                    separator: field.separator.clone(),
                    original: None,
                })
                .collect(),
        ),
        ExprKind::Nil => ExprKind::Nil,
        ExprKind::Bool(value) => ExprKind::Bool(*value),
        ExprKind::Integer(value) => ExprKind::Integer(*value),
        ExprKind::Float(value) => ExprKind::Float(value.clone()),
        ExprKind::String(value) => ExprKind::String(value.clone()),
        ExprKind::Varargs => ExprKind::Varargs,
        ExprKind::Var(name) => ExprKind::Var(name.clone()),
        ExprKind::Function(function) => ExprKind::Function(function.clone()),
        ExprKind::Raw(text) => ExprKind::Raw(text.clone()),
    };
    make_expr(&expression.span, kind)
}

fn specialize_arrays(script: &Script) -> Script {
    let mut result = script.clone();
    for statement in &mut result.statements {
        specialize_arrays_stmt(statement);
    }
    result
}

fn has_pure_array_table_expr(expression: &Expr) -> bool {
    if is_pure_array_table(expression) {
        return true;
    }
    match &expression.kind {
        ExprKind::Paren(inner) => has_pure_array_table_expr(inner),
        ExprKind::Field { object, .. } => has_pure_array_table_expr(object),
        ExprKind::Index { object, index } => {
            has_pure_array_table_expr(object)
                || has_pure_array_table_expr(index)
        }
        ExprKind::Unary { operand, .. } => has_pure_array_table_expr(operand),
        ExprKind::Binary { lhs, rhs, .. } => {
            has_pure_array_table_expr(lhs) || has_pure_array_table_expr(rhs)
        }
        ExprKind::Call { callee, args } => {
            has_pure_array_table_expr(callee)
                || args.iter().any(has_pure_array_table_expr)
        }
        ExprKind::MethodCall { object, args, .. } => {
            has_pure_array_table_expr(object)
                || args.iter().any(has_pure_array_table_expr)
        }
        ExprKind::Table(fields) => {
            fields.iter().any(|field| match &field.kind {
                TableFieldKind::Named(_, value)
                | TableFieldKind::Positional(value) => {
                    has_pure_array_table_expr(value)
                }
                TableFieldKind::Keyed(key, value) => {
                    has_pure_array_table_expr(key)
                        || has_pure_array_table_expr(value)
                }
            })
        }
        ExprKind::Function(_) => false,
        ExprKind::Nil
        | ExprKind::Bool(_)
        | ExprKind::Integer(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Varargs
        | ExprKind::Var(_)
        | ExprKind::Raw(_) => false,
    }
}

fn has_pure_array_table_stmt(statement: &Stmt) -> bool {
    match &statement.kind {
        StmtKind::Local { values, .. } | StmtKind::Return(values) => {
            values.iter().any(has_pure_array_table_expr)
        }
        StmtKind::Assign { targets, values } => {
            targets.iter().any(has_pure_array_table_expr)
                || values.iter().any(has_pure_array_table_expr)
        }
        StmtKind::Call(expr) => has_pure_array_table_expr(expr),
        StmtKind::While { condition, .. }
        | StmtKind::Repeat { condition, .. } => {
            has_pure_array_table_expr(condition)
        }
        StmtKind::NumericFor {
            start, limit, step, ..
        } => {
            has_pure_array_table_expr(start)
                || has_pure_array_table_expr(limit)
                || step.as_ref().is_some_and(has_pure_array_table_expr)
        }
        StmtKind::GenericFor { iterator, .. } => {
            has_pure_array_table_expr(iterator)
        }
        StmtKind::If { branches, .. } => branches
            .iter()
            .any(|branch| has_pure_array_table_expr(&branch.condition)),
        StmtKind::Break
        | StmtKind::Do(_)
        | StmtKind::LocalFunction { .. }
        | StmtKind::Function { .. }
        | StmtKind::Raw(_) => false,
    }
}
fn specialize_arrays_stmt(statement: &mut Stmt) {
    if !has_pure_array_table_stmt(statement) {
        return;
    }
    let kind = match &statement.kind {
        StmtKind::Local { names, values } => StmtKind::Local {
            names: names.clone(),
            values: values
                .iter()
                .map(|expr| specialize_array_expr(expr, false))
                .collect(),
        },
        StmtKind::Assign { targets, values } => StmtKind::Assign {
            targets: targets
                .iter()
                .map(|expr| specialize_array_expr(expr, false))
                .collect(),
            values: values
                .iter()
                .map(|expr| specialize_array_expr(expr, false))
                .collect(),
        },
        StmtKind::Call(expr) => {
            StmtKind::Call(specialize_array_expr(expr, false))
        }
        StmtKind::Return(values) => StmtKind::Return(
            values
                .iter()
                .map(|expr| specialize_array_expr(expr, false))
                .collect(),
        ),
        StmtKind::While {
            lead,
            condition,
            tail,
            body,
        } => StmtKind::While {
            lead: lead.clone(),
            condition: specialize_array_expr(condition, false),
            tail: tail.clone(),
            body: body.clone(),
        },
        StmtKind::Repeat { body, condition } => StmtKind::Repeat {
            body: body.clone(),
            condition: specialize_array_expr(condition, false),
        },
        StmtKind::NumericFor {
            lead,
            name,
            start,
            limit,
            step,
            tail,
            body,
        } => StmtKind::NumericFor {
            lead: lead.clone(),
            name: name.clone(),
            start: specialize_array_expr(start, false),
            limit: specialize_array_expr(limit, false),
            step: step.as_ref().map(|expr| specialize_array_expr(expr, false)),
            tail: tail.clone(),
            body: body.clone(),
        },
        StmtKind::GenericFor {
            lead,
            names,
            iterator,
            tail,
            body,
        } => StmtKind::GenericFor {
            lead: lead.clone(),
            names: names.clone(),
            iterator: specialize_array_expr(iterator, false),
            tail: tail.clone(),
            body: body.clone(),
        },
        StmtKind::If {
            branches,
            else_body,
        } => StmtKind::If {
            branches: branches
                .iter()
                .map(|branch| Branch {
                    span: branch.span.clone(),
                    keyword: branch.keyword.clone(),
                    lead: branch.lead.clone(),
                    condition: specialize_array_expr(&branch.condition, false),
                    tail: branch.tail.clone(),
                    body: branch.body.clone(),
                })
                .collect(),
            else_body: else_body.clone(),
        },
        StmtKind::Break
        | StmtKind::Do(_)
        | StmtKind::LocalFunction { .. }
        | StmtKind::Function { .. }
        | StmtKind::Raw(_) => return,
    };
    statement.kind = kind;
    statement.original = None;
}

fn specialize_array_expr(expression: &Expr, inside_setmetatable: bool) -> Expr {
    if let ExprKind::Table(fields) = &expression.kind {
        if !inside_setmetatable && is_pure_array_table(expression) {
            let marker = make_expr(
                &expression.span,
                ExprKind::Table(vec![TableField {
                    span: expression.span.clone(),
                    kind: TableFieldKind::Named(
                        "__noctalia_array".to_owned(),
                        make_expr(&expression.span, ExprKind::Bool(true)),
                    ),
                    leading: " ".to_owned(),
                    separator: String::new(),
                    original: None,
                }]),
            );
            let table =
                make_expr(&expression.span, ExprKind::Table(fields.clone()));
            return make_expr(
                &expression.span,
                ExprKind::Call {
                    callee: Box::new(make_expr(
                        &expression.span,
                        ExprKind::Var("setmetatable".to_owned()),
                    )),
                    args: vec![table, marker],
                },
            );
        }
    }
    let kind = match &expression.kind {
        ExprKind::Paren(inner) => {
            ExprKind::Paren(Box::new(specialize_array_expr(inner, false)))
        }
        ExprKind::Field {
            object,
            field,
            separator,
        } => ExprKind::Field {
            object: Box::new(specialize_array_expr(object, false)),
            field: field.clone(),
            separator: separator.clone(),
        },
        ExprKind::Index { object, index } => ExprKind::Index {
            object: Box::new(specialize_array_expr(object, false)),
            index: Box::new(specialize_array_expr(index, false)),
        },
        ExprKind::Unary { op, operand } => ExprKind::Unary {
            op: op.clone(),
            operand: Box::new(specialize_array_expr(operand, false)),
        },
        ExprKind::Binary { lhs, op, rhs } => ExprKind::Binary {
            lhs: Box::new(specialize_array_expr(lhs, false)),
            op: op.clone(),
            rhs: Box::new(specialize_array_expr(rhs, false)),
        },
        ExprKind::Call { callee, args } => {
            let marked = callee.global_name() == Some("setmetatable")
                && args.len() == 2
                && args.get(1..2).is_some_and(|slice| {
                    slice.first().is_some_and(is_array_marker_table)
                });
            ExprKind::Call {
                callee: Box::new(specialize_array_expr(callee, false)),
                args: args
                    .iter()
                    .enumerate()
                    .map(|(index, arg)| {
                        specialize_array_expr(arg, marked && index == 0)
                    })
                    .collect(),
            }
        }
        ExprKind::MethodCall {
            object,
            method,
            args,
        } => ExprKind::MethodCall {
            object: Box::new(specialize_array_expr(object, false)),
            method: method.clone(),
            args: args
                .iter()
                .map(|arg| specialize_array_expr(arg, false))
                .collect(),
        },
        ExprKind::Table(fields) => ExprKind::Table(
            fields
                .iter()
                .map(|field| TableField {
                    span: field.span.clone(),
                    kind: match &field.kind {
                        TableFieldKind::Named(name, value) => {
                            TableFieldKind::Named(
                                name.clone(),
                                specialize_array_expr(value, false),
                            )
                        }
                        TableFieldKind::Keyed(key, value) => {
                            TableFieldKind::Keyed(
                                specialize_array_expr(key, false),
                                specialize_array_expr(value, false),
                            )
                        }
                        TableFieldKind::Positional(value) => {
                            TableFieldKind::Positional(specialize_array_expr(
                                value, false,
                            ))
                        }
                    },
                    leading: field.leading.clone(),
                    separator: field.separator.clone(),
                    original: None,
                })
                .collect(),
        ),
        ExprKind::Nil => ExprKind::Nil,
        ExprKind::Bool(value) => ExprKind::Bool(*value),
        ExprKind::Integer(value) => ExprKind::Integer(*value),
        ExprKind::Float(value) => ExprKind::Float(value.clone()),
        ExprKind::String(value) => ExprKind::String(value.clone()),
        ExprKind::Varargs => ExprKind::Varargs,
        ExprKind::Var(name) => ExprKind::Var(name.clone()),
        ExprKind::Function(function) => ExprKind::Function(function.clone()),
        ExprKind::Raw(text) => ExprKind::Raw(text.clone()),
    };
    make_expr(&expression.span, kind)
}

fn has_is_array_call_script(script: &Script) -> bool {
    let mut statements = Vec::new();
    collect_script_stmts(script, &mut statements);
    statements
        .iter()
        .any(|statement| has_is_array_call_stmt(statement))
}

fn has_is_array_call_stmt(statement: &Stmt) -> bool {
    match &statement.kind {
        StmtKind::Local { values, .. } | StmtKind::Return(values) => {
            values.iter().any(has_is_array_call_expr)
        }
        StmtKind::Assign { targets, values } => {
            targets.iter().any(has_is_array_call_expr)
                || values.iter().any(has_is_array_call_expr)
        }
        StmtKind::Call(expr) => has_is_array_call_expr(expr),
        StmtKind::While { condition, .. }
        | StmtKind::Repeat { condition, .. } => {
            has_is_array_call_expr(condition)
        }
        StmtKind::NumericFor {
            start, limit, step, ..
        } => {
            has_is_array_call_expr(start)
                || has_is_array_call_expr(limit)
                || step.as_ref().is_some_and(has_is_array_call_expr)
        }
        StmtKind::GenericFor { iterator, .. } => {
            has_is_array_call_expr(iterator)
        }
        StmtKind::If { branches, .. } => branches
            .iter()
            .any(|branch| has_is_array_call_expr(&branch.condition)),
        StmtKind::LocalFunction { .. }
        | StmtKind::Function { .. }
        | StmtKind::Break
        | StmtKind::Do(_)
        | StmtKind::Raw(_) => false,
    }
}

fn has_is_array_call_expr(expression: &Expr) -> bool {
    match &expression.kind {
        ExprKind::Call { callee, args } => {
            callee.global_name() == Some("is_array")
                || callee.global_name().is_none()
                    && args.iter().any(has_is_array_call_expr)
        }
        ExprKind::Paren(inner) => has_is_array_call_expr(inner),
        ExprKind::Field { object, .. } => has_is_array_call_expr(object),
        ExprKind::Index { object, index } => {
            has_is_array_call_expr(object) || has_is_array_call_expr(index)
        }
        ExprKind::Unary { operand, .. } => has_is_array_call_expr(operand),
        ExprKind::Binary { lhs, rhs, .. } => {
            has_is_array_call_expr(lhs) || has_is_array_call_expr(rhs)
        }
        ExprKind::MethodCall { object, args, .. } => {
            has_is_array_call_expr(object)
                || args.iter().any(has_is_array_call_expr)
        }
        ExprKind::Table(fields) => {
            fields.iter().any(|field| match &field.kind {
                TableFieldKind::Named(_, value)
                | TableFieldKind::Positional(value) => {
                    has_is_array_call_expr(value)
                }
                TableFieldKind::Keyed(key, value) => {
                    has_is_array_call_expr(key) || has_is_array_call_expr(value)
                }
            })
        }
        ExprKind::Nil
        | ExprKind::Bool(_)
        | ExprKind::Integer(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Varargs
        | ExprKind::Var(_)
        | ExprKind::Function(_)
        | ExprKind::Raw(_) => false,
    }
}
