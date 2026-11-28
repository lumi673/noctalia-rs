use super::ast::{
    Block, Branch, Expr, ExprKind, Function, Script, Span, Stmt, StmtKind,
    TableField, TableFieldKind,
};

pub(crate) fn parse_script(source: &str, file: &str) -> Script {
    let mut parser = Parser {
        source,
        file: file.to_owned(),
        pos: 0,
    };
    parser.parse_script()
}

struct Parser<'a> {
    source: &'a str,
    file: String,
    pos: usize,
}

const KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function",
    "if", "in", "local", "nil", "not", "or", "repeat", "return", "then",
    "true", "until", "while",
];

const fn is_identifier_start(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphabetic()
}

const fn is_identifier_continue(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric()
}

fn precedence(operator: &str) -> u8 {
    match operator {
        "or" => 1,
        "and" => 2,
        "==" | "~=" | "<" | "<=" | ">" | ">=" => 3,
        ".." => 4,
        "+" | "-" => 5,
        "*" | "/" | "//" | "%" => 6,
        "^" => 8,
        _ => 0,
    }
}

impl<'a> Parser<'a> {
    fn parse_script(&mut self) -> Script {
        let mut statements = Vec::new();
        loop {
            let trivia = self.skip_trivia();
            let leading = self.emit_trivia(&trivia, &mut statements);
            if self.pos >= self.source.len() {
                return Script {
                    file: Some(self.file.clone()),
                    statements,
                    trailing: leading,
                };
            }
            let statement_start = self.pos;
            match self.parse_stmt() {
                Ok(mut statement) => {
                    statement.leading = leading;
                    statement.trailing = self.consume_inline_trailer();
                    statements.push(statement);
                }
                Err(()) => {
                    self.pos = statement_start;
                    let mut statement = self.consume_raw_statement(&["end"]);
                    statement.leading = leading;
                    statement.trailing = self.consume_inline_trailer();
                    statements.push(statement);
                }
            }
        }
    }

    fn emit_trivia(&self, trivia: &str, statements: &mut Vec<Stmt>) -> String {
        let mut pending = String::new();
        for chunk in trivia.split_inclusive('\n') {
            let Some(index) = chunk.find("--") else {
                pending.push_str(chunk);
                continue;
            };
            pending.push_str(chunk.get(..index).unwrap_or_default());
            let body = chunk.get(index..).unwrap_or_default();
            let text = body.trim_end_matches('\n').trim_end_matches('\r');
            let newline = body.get(text.len()..).unwrap_or_default();
            statements.push(Stmt {
                span: self.span(self.pos, self.pos),
                kind: StmtKind::Raw(text.to_owned()),
                leading: std::mem::take(&mut pending),
                trailing: newline.to_owned(),
                original: None,
            });
        }
        pending
    }

    fn byte(&self, offset: usize) -> Option<u8> {
        self.source.as_bytes().get(offset).copied()
    }

    fn slice(&self, start: usize, end: usize) -> String {
        self.source.get(start..end).unwrap_or_default().to_owned()
    }

    fn rest(&self) -> &str {
        self.source.get(self.pos..).unwrap_or_default()
    }

    fn span(&self, start: usize, end: usize) -> Span {
        let before = self.source.get(..start).unwrap_or(self.source);
        let mut line = 1usize;
        let mut column = 1usize;
        for character in before.chars() {
            if character == '\n' {
                line = line.saturating_add(1);
                column = 1;
            } else {
                column = column.saturating_add(1);
            }
        }
        let _ = end;
        Span::new(Some(self.file.clone()), line, column)
    }

    fn advance(&mut self, count: usize) {
        self.pos = self.pos.saturating_add(count);
    }

    fn consume_exact(&mut self, text: &str) -> bool {
        if self.rest().starts_with(text) {
            self.advance(text.len());
            true
        } else {
            false
        }
    }

    fn consume_word(&mut self, word: &str) -> bool {
        let end = self.pos.saturating_add(word.len());
        if self.source.get(self.pos..end) != Some(word) {
            return false;
        }
        if self.byte(end).is_some_and(is_identifier_continue) {
            return false;
        }
        self.pos = end;
        true
    }

    fn at_word(&self, word: &str) -> bool {
        let end = self.pos.saturating_add(word.len());
        self.source.get(self.pos..end) == Some(word)
            && !self.byte(end).is_some_and(is_identifier_continue)
    }

    fn at_any_word(&self, words: &[&str]) -> bool {
        words.iter().any(|word| self.at_word(word))
    }

    fn skip_spaces(&mut self) -> String {
        let start = self.pos;
        while let Some(byte) = self.byte(self.pos) {
            if byte == b' ' || byte == b'\t' || byte == b'\r' {
                self.advance(1);
            } else {
                break;
            }
        }
        self.slice(start, self.pos)
    }

    fn skip_trivia(&mut self) -> String {
        let start = self.pos;
        loop {
            let Some(byte) = self.byte(self.pos) else {
                break;
            };
            if byte == b'-'
                && self.byte(self.pos.saturating_add(1)) == Some(b'-')
            {
                while let Some(next) = self.byte(self.pos) {
                    if next == b'\n' {
                        break;
                    }
                    self.advance(1);
                }
                continue;
            }
            if byte.is_ascii_whitespace() {
                self.advance(1);
                continue;
            }
            break;
        }
        self.slice(start, self.pos)
    }

    fn skip_block_trivia(&mut self) -> String {
        let _ = self.skip_trivia();
        let mut consumed = String::new();
        loop {
            if self.consume_exact(";") {
                let more = self.skip_trivia();
                consumed.push_str(&more);
                continue;
            }
            break;
        }
        consumed
    }

    fn consume_inline_trailer(&mut self) -> String {
        let start = self.pos;
        let _ = self.skip_spaces();
        if self.byte(self.pos) == Some(b'-')
            && self.byte(self.pos.saturating_add(1)) == Some(b'-')
        {
            while let Some(byte) = self.byte(self.pos) {
                if byte == b'\n' {
                    break;
                }
                self.advance(1);
            }
        }
        self.slice(start, self.pos)
    }

    fn parse_identifier(&mut self) -> Result<String, ()> {
        let Some(byte) = self.byte(self.pos) else {
            return Err(());
        };
        if !is_identifier_start(byte) {
            return Err(());
        }
        let start = self.pos;
        self.advance(1);
        while let Some(next) = self.byte(self.pos) {
            if is_identifier_continue(next) {
                self.advance(1);
            } else {
                break;
            }
        }
        let name = self.slice(start, self.pos);
        if KEYWORDS.contains(&name.as_str()) {
            return Err(());
        }
        Ok(name)
    }

    fn parse_block(&mut self, terminators: &[&str]) -> Result<Block, ()> {
        let start = self.pos;
        let mut statements = Vec::new();
        loop {
            let trivia = self.skip_block_trivia();
            let leading = self.emit_trivia(&trivia, &mut statements);
            if self.at_any_word(terminators) {
                return Ok(Block {
                    span: self.span(start, self.pos),
                    statements,
                    close_leading: leading,
                });
            }
            if self.pos >= self.source.len() {
                return Err(());
            }
            let statement_start = self.pos;
            match self.parse_stmt() {
                Ok(mut statement) => {
                    statement.leading = leading;
                    statement.trailing = self.consume_inline_trailer();
                    statements.push(statement);
                }
                Err(()) => {
                    self.pos = statement_start;
                    let mut statement = self.consume_raw_statement(terminators);
                    statement.leading = leading;
                    statement.trailing = self.consume_inline_trailer();
                    statements.push(statement);
                    if self.at_any_word(terminators) {
                        continue;
                    }
                }
            }
        }
    }

    fn consume_raw_statement(&mut self, terminators: &[&str]) -> Stmt {
        let start = self.pos;
        loop {
            let Some(byte) = self.byte(self.pos) else {
                break;
            };
            if byte == b'\n' || byte == b';' {
                break;
            }
            if byte == b'"' || byte == b'\'' {
                if self.scan_string().is_ok() {
                    continue;
                }
            }
            if self.at_any_word(terminators) {
                break;
            }
            self.advance(1);
        }
        if self.pos == start {
            self.advance(1);
        }
        let text = self.slice(start, self.pos);
        Stmt {
            span: self.span(start, self.pos),
            kind: StmtKind::Raw(text.clone()),
            leading: String::new(),
            trailing: String::new(),
            original: Some(text),
        }
    }

    fn parse_stmt(&mut self) -> Result<Stmt, ()> {
        let start = self.pos;
        let kind = if self.consume_word("local") {
            self.parse_local(start)?
        } else if self.consume_word("function") {
            self.parse_global_function(start)?
        } else if self.consume_word("if") {
            self.parse_if(start)?
        } else if self.consume_word("do") {
            let body = self.parse_block(&["end"])?;
            if !self.consume_word("end") {
                return Err(());
            }
            StmtKind::Do(body)
        } else if self.consume_word("while") {
            self.parse_while(start)?
        } else if self.consume_word("repeat") {
            let body = self.parse_block(&["until"])?;
            if !self.consume_word("until") {
                return Err(());
            }
            let _ = self.skip_spaces();
            let condition = self.parse_expr()?;
            StmtKind::Repeat { body, condition }
        } else if self.consume_word("for") {
            self.parse_for(start)?
        } else if self.consume_word("return") {
            let _ = self.skip_spaces();
            StmtKind::Return(self.parse_expr_list()?)
        } else if self.consume_word("break") {
            StmtKind::Break
        } else {
            self.parse_expression_statement()?
        };

        if self.byte(self.pos) == Some(b';') {
            self.advance(1);
        }
        Ok(Stmt {
            span: self.span(start, self.pos),
            kind,
            leading: String::new(),
            trailing: String::new(),
            original: Some(self.slice(start, self.pos)),
        })
    }

    fn parse_local(&mut self, start: usize) -> Result<StmtKind, ()> {
        let _ = self.skip_spaces();
        let function_start = self.pos;
        if self.consume_word("function") {
            let _ = self.skip_spaces();
            let name = self.parse_identifier()?;
            let function =
                self.parse_function_after_keyword(function_start, name)?;
            return Ok(StmtKind::LocalFunction { function });
        }
        let mut names = Vec::new();
        loop {
            let _ = self.skip_spaces();
            names.push(self.parse_identifier()?);
            let save = self.pos;
            let _ = self.skip_spaces();
            if !self.consume_exact(",") {
                self.pos = save;
                break;
            }
        }
        let _ = self.skip_spaces();
        let mut values = Vec::new();
        if self.consume_exact("=") {
            let _ = self.skip_spaces();
            values = self.parse_expr_list()?;
        }
        let _ = start;
        Ok(StmtKind::Local { names, values })
    }

    fn parse_global_function(&mut self, start: usize) -> Result<StmtKind, ()> {
        let _ = self.skip_spaces();
        let target = self.parse_var_path()?;
        let name = target.global_name().unwrap_or_default().to_owned();
        let function = self.parse_function_after_keyword(start, name)?;
        Ok(StmtKind::Function { target, function })
    }

    fn parse_var_path(&mut self) -> Result<Expr, ()> {
        let start = self.pos;
        let mut expr = Expr {
            span: self.span(start, self.pos),
            kind: ExprKind::Var(self.parse_identifier()?),
            original: None,
        };
        loop {
            let save = self.pos;
            let _ = self.skip_spaces();
            if self.consume_exact(".") {
                let _ = self.skip_spaces();
                let field = self.parse_identifier()?;
                expr = Expr {
                    span: self.span(start, self.pos),
                    kind: ExprKind::Field {
                        object: Box::new(expr),
                        field,
                        separator: ".".to_owned(),
                    },
                    original: Some(self.slice(start, self.pos)),
                };
                continue;
            }
            self.pos = save;
            break;
        }
        expr.span = self.span(start, self.pos);
        expr.original = Some(self.slice(start, self.pos));
        Ok(expr)
    }

    fn parse_function_after_keyword(
        &mut self,
        start: usize,
        name: String,
    ) -> Result<Function, ()> {
        let _ = self.skip_spaces();
        if !self.consume_exact("(") {
            return Err(());
        }
        let mut params = Vec::new();
        let mut varargs = false;
        loop {
            let _ = self.skip_spaces();
            if self.consume_exact(")") {
                break;
            }
            if self.consume_exact("...") {
                varargs = true;
            } else {
                params.push(self.parse_identifier()?);
            }
            let _ = self.skip_spaces();
            if self.consume_exact(",") {
                continue;
            }
            if self.consume_exact(")") {
                break;
            }
            return Err(());
        }
        let _ = self.skip_spaces();
        let body = self.parse_block(&["end"])?;
        if !self.consume_word("end") {
            return Err(());
        }
        let text = self.slice(start, self.pos);
        Ok(Function {
            name,
            span: self.span(start, self.pos),
            params,
            varargs,
            body,
            original: Some(text),
        })
    }

    fn parse_if(&mut self, start: usize) -> Result<StmtKind, ()> {
        let _ = self.skip_spaces();
        let mut branches = Vec::new();
        let mut keyword = "if".to_owned();
        loop {
            let branch_start = self.pos.saturating_sub(keyword.len());
            let lead = self.skip_spaces();
            let condition = self.parse_expr()?;
            let tail = self.skip_spaces();
            if !self.consume_word("then") {
                return Err(());
            }
            let body = self.parse_block(&["elseif", "else", "end"])?;
            branches.push(Branch {
                span: self.span(branch_start, self.pos),
                keyword: keyword.clone(),
                lead,
                condition,
                tail,
                body,
            });
            if !self.consume_word("elseif") {
                break;
            }
            keyword = "elseif".to_owned();
        }
        let mut else_body = None;
        if self.consume_word("else") {
            else_body = Some(self.parse_block(&["end"])?);
        }
        if !self.consume_word("end") {
            return Err(());
        }
        let _ = start;
        Ok(StmtKind::If {
            branches,
            else_body,
        })
    }

    fn parse_while(&mut self, start: usize) -> Result<StmtKind, ()> {
        let lead = self.skip_spaces();
        let condition = self.parse_expr()?;
        let tail = self.skip_spaces();
        if !self.consume_word("do") {
            return Err(());
        }
        let body = self.parse_block(&["end"])?;
        if !self.consume_word("end") {
            return Err(());
        }
        let _ = start;
        Ok(StmtKind::While {
            lead,
            condition,
            tail,
            body,
        })
    }

    fn parse_for(&mut self, start: usize) -> Result<StmtKind, ()> {
        let lead = self.skip_spaces();
        let first = self.parse_identifier()?;
        let save = self.pos;
        let _ = self.skip_spaces();
        if self.consume_exact("=") {
            let _ = self.skip_spaces();
            let start_expr = self.parse_expr()?;
            let _ = self.skip_spaces();
            if !self.consume_exact(",") {
                return Err(());
            }
            let _ = self.skip_spaces();
            let limit = self.parse_expr()?;
            let save_step = self.pos;
            let _ = self.skip_spaces();
            let step = if self.consume_exact(",") {
                let _ = self.skip_spaces();
                Some(self.parse_expr()?)
            } else {
                self.pos = save_step;
                None
            };
            let tail = self.skip_spaces();
            if !self.consume_word("do") {
                return Err(());
            }
            let body = self.parse_block(&["end"])?;
            if !self.consume_word("end") {
                return Err(());
            }
            return Ok(StmtKind::NumericFor {
                lead,
                name: first,
                start: start_expr,
                limit,
                step,
                tail,
                body,
            });
        }
        self.pos = save;
        let mut names = vec![first];
        loop {
            let _ = self.skip_spaces();
            if !self.consume_exact(",") {
                break;
            }
            let _ = self.skip_spaces();
            names.push(self.parse_identifier()?);
        }
        let _ = self.skip_spaces();
        if !self.consume_word("in") {
            return Err(());
        }
        let _ = self.skip_spaces();
        let iterator = self.parse_expr()?;
        let tail = self.skip_spaces();
        if !self.consume_word("do") {
            return Err(());
        }
        let body = self.parse_block(&["end"])?;
        if !self.consume_word("end") {
            return Err(());
        }
        let _ = start;
        Ok(StmtKind::GenericFor {
            lead,
            names,
            iterator,
            tail,
            body,
        })
    }

    fn parse_expression_statement(&mut self) -> Result<StmtKind, ()> {
        let first = self.parse_expr()?;
        let mut targets = vec![first];
        let save = self.pos;
        let _ = self.skip_spaces();
        while self.consume_exact(",") {
            let _ = self.skip_spaces();
            targets.push(self.parse_expr()?);
            let _ = self.skip_spaces();
        }
        if self.consume_exact("=") {
            let _ = self.skip_spaces();
            let values = self.parse_expr_list()?;
            return Ok(StmtKind::Assign { targets, values });
        }
        self.pos = save;
        if targets.len() != 1 {
            return Err(());
        }
        let Some(expr) = targets.pop() else {
            return Err(());
        };
        if matches!(
            expr.kind,
            ExprKind::Call { .. } | ExprKind::MethodCall { .. }
        ) {
            return Ok(StmtKind::Call(expr));
        }
        Err(())
    }

    fn parse_expr_list(&mut self) -> Result<Vec<Expr>, ()> {
        let mut values = Vec::new();
        loop {
            let _ = self.skip_spaces();
            if self.at_expr_stop() {
                break;
            }
            values.push(self.parse_expr()?);
            let save = self.pos;
            let _ = self.skip_spaces();
            if self.consume_exact(",") {
                continue;
            }
            self.pos = save;
            break;
        }
        Ok(values)
    }

    fn at_expr_stop(&self) -> bool {
        if self.pos >= self.source.len() {
            return true;
        }
        if self.at_any_word(&["end", "else", "elseif", "until", "then", "do"]) {
            return true;
        }
        matches!(
            self.byte(self.pos),
            Some(b'\n') | Some(b';') | Some(b')') | Some(b'}') | Some(b',')
        ) || self.rest().starts_with("--")
    }

    fn parse_expr(&mut self) -> Result<Expr, ()> {
        let start = self.pos;
        self.parse_binary(start, 1)
    }

    fn parse_binary(&mut self, start: usize, minimum: u8) -> Result<Expr, ()> {
        let mut lhs = self.parse_unary()?;
        loop {
            let save = self.pos;
            let _ = self.skip_spaces();
            let Some(operator) = self.peek_operator() else {
                self.pos = save;
                break;
            };
            let level = precedence(&operator);
            if level == 0 || level < minimum {
                self.pos = save;
                break;
            }
            self.advance(operator.len());
            let _ = self.skip_spaces();
            let next_minimum = if operator == "^" || operator == ".." {
                level
            } else {
                level.saturating_add(1)
            };
            let rhs = self.parse_binary(self.pos, next_minimum)?;
            lhs = Expr {
                span: self.span(start, self.pos),
                kind: ExprKind::Binary {
                    lhs: Box::new(lhs),
                    op: operator,
                    rhs: Box::new(rhs),
                },
                original: Some(self.slice(start, self.pos)),
            };
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<Expr, ()> {
        let start = self.pos;
        if self.consume_word("not") {
            let _ = self.skip_spaces();
            let operand = self.parse_unary()?;
            return Ok(Expr {
                span: self.span(start, self.pos),
                kind: ExprKind::Unary {
                    op: "not".to_owned(),
                    operand: Box::new(operand),
                },
                original: Some(self.slice(start, self.pos)),
            });
        }
        if self.byte(start) == Some(b'-')
            && self.byte(start.saturating_add(1)) != Some(b'-')
        {
            self.advance(1);
            let _ = self.skip_spaces();
            let operand = self.parse_unary()?;
            return Ok(Expr {
                span: self.span(start, self.pos),
                kind: ExprKind::Unary {
                    op: "-".to_owned(),
                    operand: Box::new(operand),
                },
                original: Some(self.slice(start, self.pos)),
            });
        }
        if self.byte(start) == Some(b'#') {
            self.advance(1);
            let _ = self.skip_spaces();
            let operand = self.parse_unary()?;
            return Ok(Expr {
                span: self.span(start, self.pos),
                kind: ExprKind::Unary {
                    op: "#".to_owned(),
                    operand: Box::new(operand),
                },
                original: Some(self.slice(start, self.pos)),
            });
        }
        self.parse_postfix(start)
    }

    fn parse_postfix(&mut self, start: usize) -> Result<Expr, ()> {
        let mut expr = self.parse_primary(start)?;
        loop {
            let save = self.pos;
            let _ = self.skip_spaces();
            if self.consume_exact(".") {
                let _ = self.skip_spaces();
                match self.parse_identifier() {
                    Ok(field) => {
                        expr = Expr {
                            span: self.span(start, self.pos),
                            kind: ExprKind::Field {
                                object: Box::new(expr),
                                field,
                                separator: ".".to_owned(),
                            },
                            original: Some(self.slice(start, self.pos)),
                        };
                        continue;
                    }
                    Err(()) => {
                        self.pos = save;
                        break;
                    }
                }
            }
            if self.consume_exact(":") {
                let _ = self.skip_spaces();
                let Ok(method) = self.parse_identifier() else {
                    self.pos = save;
                    break;
                };
                let Ok(args) = self.parse_call_args() else {
                    self.pos = save;
                    break;
                };
                expr = Expr {
                    span: self.span(start, self.pos),
                    kind: ExprKind::MethodCall {
                        object: Box::new(expr),
                        method,
                        args,
                    },
                    original: Some(self.slice(start, self.pos)),
                };
                continue;
            }
            if self.consume_exact("[") {
                let Ok(index) = self.parse_expr() else {
                    self.pos = save;
                    break;
                };
                let _ = self.skip_spaces();
                if !self.consume_exact("]") {
                    self.pos = save;
                    break;
                }
                expr = Expr {
                    span: self.span(start, self.pos),
                    kind: ExprKind::Index {
                        object: Box::new(expr),
                        index: Box::new(index),
                    },
                    original: Some(self.slice(start, self.pos)),
                };
                continue;
            }
            if self.consume_exact("(") {
                let Ok(args) = self.parse_call_args_after_open() else {
                    self.pos = save;
                    break;
                };
                expr = Expr {
                    span: self.span(start, self.pos),
                    kind: ExprKind::Call {
                        callee: Box::new(expr),
                        args,
                    },
                    original: Some(self.slice(start, self.pos)),
                };
                continue;
            }
            if matches!(
                self.byte(self.pos),
                Some(b'"') | Some(b'\'') | Some(b'{')
            ) {
                let Ok(argument) = self.parse_primary(self.pos) else {
                    self.pos = save;
                    break;
                };
                expr = Expr {
                    span: self.span(start, self.pos),
                    kind: ExprKind::Call {
                        callee: Box::new(expr),
                        args: vec![argument],
                    },
                    original: Some(self.slice(start, self.pos)),
                };
                continue;
            }
            self.pos = save;
            break;
        }
        Ok(expr)
    }

    fn parse_primary(&mut self, start: usize) -> Result<Expr, ()> {
        if self.consume_word("nil") {
            return Ok(self.finish_primary(start, ExprKind::Nil));
        }
        if self.consume_word("true") {
            return Ok(self.finish_primary(start, ExprKind::Bool(true)));
        }
        if self.consume_word("false") {
            return Ok(self.finish_primary(start, ExprKind::Bool(false)));
        }
        if self.consume_exact("...") {
            return Ok(self.finish_primary(start, ExprKind::Varargs));
        }
        if matches!(self.byte(self.pos), Some(b'"') | Some(b'\'')) {
            let value = self.scan_string()?;
            return Ok(self.finish_primary(start, ExprKind::String(value)));
        }
        if self.consume_exact("{") {
            let fields = self.parse_table_after_open()?;
            return Ok(self.finish_primary(start, ExprKind::Table(fields.0)));
        }
        if self.consume_word("function") {
            let function =
                self.parse_function_after_keyword(start, String::new())?;
            return Ok(self.finish_primary(start, ExprKind::Function(function)));
        }
        if self.consume_exact("(") {
            let _ = self.skip_spaces();
            let inner = self.parse_expr()?;
            let _ = self.skip_spaces();
            if !self.consume_exact(")") {
                return Err(());
            }
            return Ok(
                self.finish_primary(start, ExprKind::Paren(Box::new(inner)))
            );
        }
        let Some(byte) = self.byte(self.pos) else {
            return Err(());
        };
        if byte.is_ascii_digit()
            || (byte == b'.'
                && self
                    .byte(self.pos.saturating_add(1))
                    .is_some_and(|next| next.is_ascii_digit()))
        {
            let kind = self.scan_number()?;
            return Ok(self.finish_primary(start, kind));
        }
        if is_identifier_start(byte) {
            let name = self.parse_identifier()?;
            return Ok(self.finish_primary(start, ExprKind::Var(name)));
        }
        let Some(character) = self.rest().chars().next() else {
            return Err(());
        };
        if matches!(character, '\n' | ';' | ')' | '}' | ',') {
            return Err(());
        }
        self.advance(character.len_utf8());
        let text = self.slice(start, self.pos);
        Ok(self.finish_primary(start, ExprKind::Raw(text)))
    }

    fn finish_primary(&self, start: usize, kind: ExprKind) -> Expr {
        Expr {
            span: self.span(start, self.pos),
            kind,
            original: Some(self.slice(start, self.pos)),
        }
    }

    fn scan_number(&mut self) -> Result<ExprKind, ()> {
        let start = self.pos;
        let mut is_float = false;
        while let Some(byte) = self.byte(self.pos) {
            if byte.is_ascii_digit() {
                self.advance(1);
                continue;
            }
            if byte == b'.'
                && self.byte(self.pos.saturating_add(1)) != Some(b'.')
            {
                is_float = true;
                self.advance(1);
                continue;
            }
            if byte == b'e' || byte == b'E' {
                is_float = true;
                self.advance(1);
                if matches!(self.byte(self.pos), Some(b'+') | Some(b'-')) {
                    self.advance(1);
                }
                continue;
            }
            if byte == b'x' || byte == b'X' {
                is_float = true;
                self.advance(1);
                continue;
            }
            break;
        }
        let text = self.slice(start, self.pos);
        if text.is_empty() {
            return Err(());
        }
        if is_float {
            return Ok(ExprKind::Float(text));
        }
        match text.parse::<i128>() {
            Ok(value) => Ok(ExprKind::Integer(value)),
            Err(_) => Ok(ExprKind::Float(text)),
        }
    }

    fn scan_string(&mut self) -> Result<String, ()> {
        let Some(quote) = self.byte(self.pos) else {
            return Err(());
        };
        if quote != b'"' && quote != b'\'' {
            return Err(());
        }
        self.advance(1);
        let mut value = String::new();
        loop {
            let Some(byte) = self.byte(self.pos) else {
                return Err(());
            };
            if byte == quote {
                self.advance(1);
                return Ok(value);
            }
            if byte == b'\\' {
                self.advance(1);
                let Some(escape) = self.byte(self.pos) else {
                    return Err(());
                };
                self.advance(1);
                match escape {
                    b'n' => value.push('\n'),
                    b'r' => value.push('\r'),
                    b't' => value.push('\t'),
                    b'a' => value.push('\u{0007}'),
                    b'b' => value.push('\u{0008}'),
                    b'f' => value.push('\u{000c}'),
                    b'v' => value.push('\u{000b}'),
                    b'0'..=b'9' => {
                        let Some(first) =
                            u32::from(escape).checked_sub(u32::from(b'0'))
                        else {
                            return Err(());
                        };
                        let mut number = first;
                        for _ in 0..2 {
                            let Some(next) = self.byte(self.pos) else {
                                break;
                            };
                            if !next.is_ascii_digit() {
                                break;
                            }
                            let Some(digit) =
                                u32::from(next).checked_sub(u32::from(b'0'))
                            else {
                                return Err(());
                            };
                            let Some(combined) = number
                                .checked_mul(10)
                                .and_then(|scaled| scaled.checked_add(digit))
                            else {
                                return Err(());
                            };
                            number = combined;
                            self.advance(1);
                        }
                        let Some(character) = char::from_u32(number) else {
                            return Err(());
                        };
                        value.push(character);
                    }
                    other => value.push(char::from(other)),
                }
                continue;
            }
            let Some(character) = self.rest().chars().next() else {
                return Err(());
            };
            value.push(character);
            self.advance(character.len_utf8());
        }
    }

    fn parse_call_args(&mut self) -> Result<Vec<Expr>, ()> {
        if self.consume_exact("(") {
            return self.parse_call_args_after_open();
        }
        if matches!(self.byte(self.pos), Some(b'"') | Some(b'\'') | Some(b'{'))
        {
            let argument = self.parse_primary(self.pos)?;
            return Ok(vec![argument]);
        }
        Err(())
    }

    fn parse_call_args_after_open(&mut self) -> Result<Vec<Expr>, ()> {
        let mut args = Vec::new();
        loop {
            let _ = self.skip_spaces();
            if self.byte(self.pos) == Some(b')') {
                break;
            }
            args.push(self.parse_expr()?);
            let _ = self.skip_spaces();
            if self.consume_exact(",") {
                continue;
            }
            if self.byte(self.pos) == Some(b')') {
                break;
            }
            return Err(());
        }
        if !self.consume_exact(")") {
            return Err(());
        }
        Ok(args)
    }

    fn parse_table_after_open(
        &mut self,
    ) -> Result<(Vec<TableField>, String), ()> {
        let mut fields = Vec::new();
        loop {
            let leading = self.skip_trivia();
            if self.consume_exact("}") {
                return Ok((fields, leading));
            }
            if self.pos >= self.source.len() {
                return Err(());
            }
            let start = self.pos;
            let kind = if let Some(name) = self.try_named_field() {
                TableFieldKind::Named(name, self.parse_expr()?)
            } else if self.consume_exact("[") {
                let key = self.parse_expr()?;
                let _ = self.skip_spaces();
                if !self.consume_exact("]") {
                    return Err(());
                }
                let _ = self.skip_spaces();
                if !self.consume_exact("=") {
                    return Err(());
                }
                let _ = self.skip_spaces();
                TableFieldKind::Keyed(key, self.parse_expr()?)
            } else {
                TableFieldKind::Positional(self.parse_expr()?)
            };
            let text = self.slice(start, self.pos);
            let separator_start = self.pos;
            let _ = self.skip_spaces();
            let separator =
                if self.consume_exact(",") || self.consume_exact(";") {
                    self.slice(separator_start, self.pos)
                } else {
                    String::new()
                };
            fields.push(TableField {
                span: self.span(start, self.pos),
                kind,
                leading,
                separator: separator.clone(),
                original: Some(text),
            });
            if !separator.is_empty() {
                continue;
            }
            let _ = self.skip_trivia();
            if self.consume_exact("}") {
                return Ok((fields, String::new()));
            }
            return Err(());
        }
    }

    fn try_named_field(&mut self) -> Option<String> {
        let save = self.pos;
        let Some(name) = self.parse_identifier().ok() else {
            self.pos = save;
            return None;
        };
        let _ = self.skip_spaces();
        if self.consume_exact("=") {
            let _ = self.skip_spaces();
            Some(name)
        } else {
            self.pos = save;
            None
        }
    }

    fn peek_operator(&self) -> Option<String> {
        if self.at_word("or") {
            return Some("or".to_owned());
        }
        if self.at_word("and") {
            return Some("and".to_owned());
        }
        const OPERATORS: &[&str] = &[
            "~=", "<=", ">=", "//", "==", "..", "+", "-", "*", "/", "%", "^",
            "<", ">",
        ];
        for operator in OPERATORS {
            if !self.rest().starts_with(operator) {
                continue;
            }
            if *operator == "-" && self.rest().starts_with("--") {
                continue;
            }
            return Some((*operator).to_owned());
        }
        None
    }
}
