//! Macro expansion for the typed frontend.
//!
//! `macro_rules!` definitions in the crate are matched against their call
//! sites and substituted before name resolution. Built-in macros stay for the
//! emitter, which already lowers `format!`, `vec!`, `json!` and `matches!` to
//! Luau. Anything the matcher cannot prove is a positioned error.

use std::collections::BTreeMap;

use proc_macro2::{Delimiter, Group, Span, TokenStream, TokenTree};
use quote::ToTokens;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::visit_mut::{self, VisitMut};
use syn::{Expr, File, Item, Stmt, Token};

use crate::Error;

#[derive(Clone)]
struct MacroDef {
    rules: Vec<Rule>,
}

#[derive(Clone)]
struct Rule {
    pattern: Vec<PatternToken>,
    template: Vec<PatternToken>,
}

#[derive(Clone, Debug)]
enum PatternToken {
    Token(TokenTree),
    Nested {
        delimiter: Delimiter,
        tokens: Vec<PatternToken>,
    },
    Meta {
        name: String,
        kind: MetaKind,
    },
    Repeat {
        pattern: Vec<PatternToken>,
        separator: Option<TokenTree>,
        once: bool,
    },
}

#[derive(Clone, Copy, Debug)]
enum MetaKind {
    Ident,
    Literal,
    Ty,
    Expr,
    Path,
    Tt,
}

type Captures = BTreeMap<String, Vec<TokenStream>>;

/// Removes `macro_rules!` definitions and expands their uses in one file.
pub(super) fn expand_file(file: &mut File) -> Result<(), Error> {
    let mut defs = BTreeMap::new();
    collect(&mut file.items, &mut defs)?;
    let mut expander = Expander {
        defs: &defs,
        error: None,
    };
    expander.visit_file_mut(file);
    match expander.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn collect(
    items: &mut Vec<Item>,
    defs: &mut BTreeMap<String, MacroDef>,
) -> Result<(), Error> {
    let mut kept = Vec::with_capacity(items.len());
    for item in std::mem::take(items) {
        match item {
            Item::Macro(item) if item.mac.path.is_ident("macro_rules") => {
                let (name, def) = parse_def(&item)?;
                defs.insert(name, def);
            }
            Item::Mod(mut module) => {
                if let Some((_, inner)) = &mut module.content {
                    collect(inner, defs)?;
                }
                kept.push(Item::Mod(module));
            }
            other => kept.push(other),
        }
    }
    *items = kept;
    Ok(())
}

fn parse_def(item: &syn::ItemMacro) -> Result<(String, MacroDef), Error> {
    let trees = item.mac.tokens.clone().into_iter().collect::<Vec<_>>();
    let name = item
        .ident
        .as_ref()
        .or_else(|| {
            trees.iter().find_map(|token| match token {
                TokenTree::Ident(ident) => Some(ident),
                _ => None,
            })
        })
        .map(ToString::to_string)
        .ok_or_else(|| Error::at(item.span(), "macro_rules! needs a name"))?;
    let body = if item.ident.is_some() {
        item.mac.tokens.clone()
    } else {
        trees
            .iter()
            .find_map(|token| match token {
                TokenTree::Group(group) => Some(group.stream()),
                _ => None,
            })
            .ok_or_else(|| {
                Error::at(item.span(), "macro_rules! needs a rule body")
            })?
    };
    let rules = parse_rules(&body, item.span())?;
    Ok((name, MacroDef { rules }))
}

fn parse_rules(tokens: &TokenStream, span: Span) -> Result<Vec<Rule>, Error> {
    let trees = tokens.clone().into_iter().collect::<Vec<_>>();
    let mut rules = Vec::new();
    let mut at = 0;
    while at < trees.len() {
        let Some((pattern, next)) = take_group(&trees, at) else {
            return Err(Error::at(span, "macro rule needs a pattern group"));
        };
        at = next;
        if !is_arrow(&trees, at) {
            return Err(Error::at(span, "macro rule needs `=>`"));
        }
        at = at.saturating_add(2);
        let Some((body, next)) = take_group(&trees, at) else {
            return Err(Error::at(span, "macro rule needs a body group"));
        };
        at = next;
        if at < trees.len() {
            if !is_punct(&trees[at], ';') {
                return Err(Error::at(span, "macro rules must end with `;`"));
            }
            at = at.saturating_add(1);
        }
        rules.push(Rule {
            pattern: parse_tokens(&pattern, span, true)?,
            template: parse_tokens(&body, span, false)?,
        });
    }
    Ok(rules)
}

fn take_group(trees: &[TokenTree], at: usize) -> Option<(TokenStream, usize)> {
    let TokenTree::Group(group) = trees.get(at)? else {
        return None;
    };
    Some((group.stream(), at.saturating_add(1)))
}

fn is_punct(token: &TokenTree, character: char) -> bool {
    matches!(token, TokenTree::Punct(punct) if punct.as_char() == character)
}

fn is_arrow(trees: &[TokenTree], at: usize) -> bool {
    trees.get(at).is_some_and(|token| is_punct(token, '='))
        && trees
            .get(at.saturating_add(1))
            .is_some_and(|token| is_punct(token, '>'))
}

fn parse_tokens(
    tokens: &TokenStream,
    span: Span,
    require_kind: bool,
) -> Result<Vec<PatternToken>, Error> {
    let trees = tokens.clone().into_iter().collect::<Vec<_>>();
    let mut result = Vec::new();
    let mut at = 0;
    while at < trees.len() {
        if !is_punct(&trees[at], '$') {
            if let TokenTree::Group(group) = &trees[at] {
                result.push(PatternToken::Nested {
                    delimiter: group.delimiter(),
                    tokens: parse_tokens(&group.stream(), span, require_kind)?,
                });
            } else {
                result.push(PatternToken::Token(trees[at].clone()));
            }
            at = at.saturating_add(1);
            continue;
        }
        at = at.saturating_add(1);
        let Some(token) = trees.get(at) else {
            return Err(Error::at(span, "macro token ends after `$`"));
        };
        match token {
            TokenTree::Group(group)
                if group.delimiter() == Delimiter::Parenthesis =>
            {
                let inner = parse_tokens(&group.stream(), span, require_kind)?;
                at = at.saturating_add(1);
                let separator = trees.get(at).filter(|token| {
                    !is_punct(token, '*') && !is_punct(token, '+')
                });
                if separator.is_some() {
                    at = at.saturating_add(1);
                }
                let once = match trees.get(at) {
                    Some(token) if is_punct(token, '+') => true,
                    Some(token) if is_punct(token, '*') => false,
                    _ => {
                        return Err(Error::at(
                            span,
                            "macro repetition needs `*` or `+`",
                        ));
                    }
                };
                at = at.saturating_add(1);
                result.push(PatternToken::Repeat {
                    pattern: inner,
                    separator: separator.cloned(),
                    once,
                });
            }
            TokenTree::Ident(name) => {
                let name = name.to_string();
                at = at.saturating_add(1);
                let kind = if trees
                    .get(at)
                    .is_some_and(|token| is_punct(token, ':'))
                {
                    at = at.saturating_add(1);
                    let Some(TokenTree::Ident(kind)) = trees.get(at) else {
                        return Err(Error::at(
                            span,
                            "macro metavariable needs a kind",
                        ));
                    };
                    let kind = match kind.to_string().as_str() {
                        "ident" => MetaKind::Ident,
                        "literal" | "lit" => MetaKind::Literal,
                        "ty" => MetaKind::Ty,
                        "expr" => MetaKind::Expr,
                        "path" => MetaKind::Path,
                        "tt" => MetaKind::Tt,
                        other => {
                            return Err(Error::at(
                                span,
                                format!(
                                    "macro fragment `{other}` is not supported"
                                ),
                            ));
                        }
                    };
                    at = at.saturating_add(1);
                    kind
                } else if require_kind {
                    return Err(Error::at(
                        span,
                        "macro pattern metavariable needs a kind",
                    ));
                } else {
                    MetaKind::Expr
                };
                result.push(PatternToken::Meta { name, kind });
            }
            _ => {
                return Err(Error::at(span, "macro `$` must name a fragment"));
            }
        }
    }
    Ok(result)
}

fn same_token(left: &TokenTree, right: &TokenTree) -> bool {
    match (left, right) {
        (TokenTree::Ident(left), TokenTree::Ident(right)) => left == right,
        (TokenTree::Punct(left), TokenTree::Punct(right)) => {
            left.as_char() == right.as_char()
        }
        (TokenTree::Literal(left), TokenTree::Literal(right)) => {
            left.to_string() == right.to_string()
        }
        (TokenTree::Group(left), TokenTree::Group(right)) => {
            left.delimiter() == right.delimiter()
        }
        _ => false,
    }
}

fn match_tokens(
    pattern: &[PatternToken],
    input: &[TokenTree],
    captures: &mut Captures,
    span: Span,
) -> Result<usize, Error> {
    let mut at = 0;
    for (index, token) in pattern.iter().enumerate() {
        match token {
            PatternToken::Token(expected) => {
                let Some(actual) = input.get(at) else {
                    return Ok(0);
                };
                if !same_token(expected, actual) {
                    return Ok(0);
                }
                at = at.saturating_add(1);
            }
            PatternToken::Nested {
                delimiter,
                tokens: inner_tokens,
            } => {
                let Some(TokenTree::Group(group)) = input.get(at) else {
                    return Ok(0);
                };
                if group.delimiter() != *delimiter {
                    return Ok(0);
                }
                let inner = group.stream().into_iter().collect::<Vec<_>>();
                let consumed =
                    match_tokens(inner_tokens, &inner, captures, span)?;
                if consumed != inner.len() {
                    return Ok(0);
                }
                at = at.saturating_add(1);
            }
            PatternToken::Meta { name, kind } => {
                let width = match kind {
                    MetaKind::Ident | MetaKind::Literal | MetaKind::Tt => {
                        let Some(token) = input.get(at) else {
                            return Ok(0);
                        };
                        let matches = match kind {
                            MetaKind::Ident => {
                                matches!(token, TokenTree::Ident(_))
                            }
                            MetaKind::Literal => {
                                matches!(token, TokenTree::Literal(_))
                            }
                            MetaKind::Tt => true,
                            _ => false,
                        };
                        if !matches {
                            return Ok(0);
                        }
                        1
                    }
                    MetaKind::Ty | MetaKind::Expr | MetaKind::Path => {
                        let boundary = pattern.get(index.saturating_add(1));
                        let mut end = at;
                        while end < input.len() {
                            if let Some(PatternToken::Token(expected)) =
                                boundary
                                && same_token(expected, &input[end])
                            {
                                break;
                            }
                            end = end.saturating_add(1);
                        }
                        end.saturating_sub(at)
                    }
                };
                if width == 0 || at.saturating_add(width) > input.len() {
                    return Ok(0);
                }
                let matched = input[at..at.saturating_add(width)]
                    .iter()
                    .cloned()
                    .collect::<TokenStream>();
                captures.entry(name.clone()).or_default().push(matched);
                at = at.saturating_add(width);
            }
            PatternToken::Repeat {
                pattern: inner,
                separator,
                once,
            } => {
                let mut start = at;
                let mut count: usize = 0;
                loop {
                    let lengths = lengths(captures, inner);
                    let remaining = &input[start..];
                    let limited =
                        separator.as_ref().map_or(remaining, |separator| {
                            let end = remaining
                                .iter()
                                .position(|token| same_token(separator, token))
                                .unwrap_or(remaining.len());
                            &remaining[..end]
                        });
                    let Some(consumed) =
                        match_tokens(inner, limited, captures, span)?
                            .checked_sub(0)
                            .filter(|value| *value > 0)
                    else {
                        restore(captures, &lengths);
                        break;
                    };
                    start = start.saturating_add(consumed);
                    count = count.saturating_add(1);
                    if let Some(separator) = separator {
                        if input
                            .get(start)
                            .is_some_and(|token| same_token(separator, token))
                        {
                            start = start.saturating_add(1);
                        } else {
                            break;
                        }
                    }
                }
                if *once && count == 0 {
                    return Err(Error::at(
                        span,
                        "macro repetition matched nothing",
                    ));
                }
                at = start;
            }
        }
    }
    Ok(at)
}

fn names(pattern: &[PatternToken]) -> Vec<String> {
    let mut result = Vec::new();
    for token in pattern {
        match token {
            PatternToken::Meta { name, .. } => {
                if !result.contains(name) {
                    result.push(name.clone());
                }
            }
            PatternToken::Nested { tokens, .. } => {
                for name in names(tokens) {
                    if !result.contains(&name) {
                        result.push(name);
                    }
                }
            }
            PatternToken::Repeat { pattern, .. } => {
                for name in names(pattern) {
                    if !result.contains(&name) {
                        result.push(name);
                    }
                }
            }
            PatternToken::Token(_) => {}
        }
    }
    result
}

fn lengths(
    captures: &Captures,
    pattern: &[PatternToken],
) -> Vec<(String, usize)> {
    names(pattern)
        .into_iter()
        .map(|name| {
            let length = captures.get(&name).map_or(0, Vec::len);
            (name, length)
        })
        .collect()
}

fn restore(captures: &mut Captures, lengths: &[(String, usize)]) {
    for (name, length) in lengths {
        if let Some(values) = captures.get_mut(name) {
            values.truncate(*length);
        }
    }
}

fn expand_stream(
    template: &[PatternToken],
    captures: &Captures,
    index: usize,
    span: Span,
) -> Result<TokenStream, Error> {
    let mut out = TokenStream::new();
    for token in template {
        match token {
            PatternToken::Token(token) => {
                let mut token = token.clone();
                respan(&mut token, span);
                out.extend(std::iter::once(token));
            }
            PatternToken::Nested {
                delimiter,
                tokens: inner_tokens,
            } => {
                let inner = expand_stream(inner_tokens, captures, index, span)?;
                let mut group = Group::new(*delimiter, inner);
                group.set_span(span);
                out.extend(std::iter::once(TokenTree::Group(group)));
            }
            PatternToken::Meta { name, .. } => {
                let Some(value) =
                    captures.get(name).and_then(|values| values.get(index))
                else {
                    return Err(Error::at(
                        span,
                        format!("macro capture `{name}` is missing"),
                    ));
                };
                out.extend(respan_stream(value.clone(), span));
            }
            PatternToken::Repeat {
                pattern, separator, ..
            } => {
                let count = names(pattern)
                    .first()
                    .and_then(|name| captures.get(name).map(Vec::len))
                    .unwrap_or(0);
                for round in 0..count {
                    if round > 0
                        && let Some(separator) = separator
                    {
                        let mut separator = separator.clone();
                        respan(&mut separator, span);
                        out.extend(std::iter::once(separator));
                    }
                    out.extend(expand_stream(pattern, captures, round, span)?);
                }
            }
        }
    }
    Ok(out)
}

fn respan_stream(stream: TokenStream, span: Span) -> TokenStream {
    stream
        .into_iter()
        .map(|mut token| {
            respan(&mut token, span);
            token
        })
        .collect()
}

fn respan(token: &mut TokenTree, span: Span) {
    match token {
        TokenTree::Group(group) => {
            let inner = respan_stream(group.stream(), span);
            let mut next = Group::new(group.delimiter(), inner);
            next.set_span(span);
            *group = next;
        }
        TokenTree::Ident(ident) => ident.set_span(span),
        TokenTree::Punct(punct) => punct.set_span(span),
        TokenTree::Literal(literal) => literal.set_span(span),
    }
}

struct Expander<'a> {
    defs: &'a BTreeMap<String, MacroDef>,
    error: Option<Error>,
}

impl Expander<'_> {
    fn expand(&mut self, mac: &syn::Macro) -> Option<TokenStream> {
        let name = mac.path.get_ident().map(ToString::to_string)?;
        let def = self.defs.get(&name)?;
        let span = mac.path.span();
        let arguments = mac.tokens.clone().into_iter().collect::<Vec<_>>();
        let mut first_error = None;
        for rule in &def.rules {
            let mut captures = Captures::new();
            let consumed = match match_tokens(
                &rule.pattern,
                &arguments,
                &mut captures,
                span,
            ) {
                Ok(consumed) => consumed,
                Err(error) => {
                    first_error.get_or_insert(error);
                    continue;
                }
            };
            if consumed != arguments.len() {
                continue;
            }
            match expand_stream(&rule.template, &captures, 0, span) {
                Ok(tokens) => return Some(respan_stream(tokens, span)),
                Err(error) => {
                    first_error.get_or_insert(error);
                    continue;
                }
            }
        }
        let error = first_error.unwrap_or_else(|| {
            Error::at(
                span,
                format!("no `macro_rules! {name}` rule matches this call"),
            )
        });
        self.error.get_or_insert(error);
        None
    }
}

impl VisitMut for Expander<'_> {
    fn visit_expr_mut(&mut self, node: &mut Expr) {
        visit_mut::visit_expr_mut(self, node);
        let Expr::Macro(mac) = node else {
            return;
        };
        let Some(tokens) = self.expand(&mac.mac) else {
            return;
        };
        match syn::parse2::<Expr>(tokens) {
            Ok(expr) => *node = expr,
            Err(error) => {
                let error = Error::from(error);
                self.error.get_or_insert(error);
            }
        }
    }
    fn visit_macro_mut(&mut self, mac: &mut syn::Macro) {
        if mac.path.is_ident("vec")
            && let Ok(mut elements) =
                Punctuated::<Expr, Token![,]>::parse_terminated
                    .parse2(mac.tokens.clone())
        {
            for element in &mut elements {
                self.visit_expr_mut(element);
            }
            mac.tokens = elements.to_token_stream();
            return;
        }
        visit_mut::visit_macro_mut(self, mac);
    }

    fn visit_stmt_mut(&mut self, node: &mut Stmt) {
        visit_mut::visit_stmt_mut(self, node);
        let Stmt::Macro(statement) = node else {
            return;
        };
        let Some(tokens) = self.expand(&statement.mac) else {
            return;
        };
        match syn::parse2::<Expr>(tokens) {
            Ok(expr) => {
                let semi = statement.semi_token;
                *node = Stmt::Expr(expr, semi);
            }
            Err(error) => {
                let error = Error::from(error);
                self.error.get_or_insert(error);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn expand(source: &str) -> String {
        let mut file = syn::parse_file(source).expect("parse");
        expand_file(&mut file).expect("expand");
        quote!(#file).to_string()
    }

    #[test]
    fn expands_a_literal_macro_call() {
        let output = expand(
            "macro_rules! op { ($value:expr) => { Op::log($value) }; } fn main() { let _ = op!(\"hi\"); }",
        );
        assert!(output.contains("Op :: log (\"hi\")"), "{output}");
        assert!(!output.contains("macro_rules"), "{output}");
    }

    #[test]
    fn expands_a_repetition() {
        let output = expand(
            "macro_rules! ops { ($($value:expr),*) => { vec![$($value),*] }; } fn main() { let _ = ops!(\"a\", \"b\"); }",
        );
        assert!(output.contains("\"a\""), "{output}");
        assert!(output.contains("\"b\""), "{output}");
    }

    #[test]
    fn unmatched_rules_are_positioned_errors() {
        let mut file = syn::parse_file(
            "macro_rules! op { ($value:expr) => { $value }; } fn main() { let _ = op!(1, 2); }",
        )
        .expect("parse");
        let error =
            expand_file(&mut file).expect_err("two arguments do not match");
        assert_eq!(error.line, 1);
    }

    #[test]
    fn ident_and_literal_fragments_check_the_token_kind() {
        let mut file = syn::parse_file(
            "macro_rules! take { ($name:ident) => { $name }; } fn main() { let _ = take!(\"not an ident\"); }",
        )
        .expect("parse");
        let error =
            expand_file(&mut file).expect_err("literal is not an ident");
        assert!(
            error.message.contains("no `macro_rules! take` rule"),
            "{error:?}"
        );
    }

    #[test]
    fn a_later_rule_can_match_after_an_earlier_repetition_error() {
        let output = expand(
            "macro_rules! one_or_two { ($($value:expr),+) => { $($value),* }; ($value:expr) => { $value }; } fn main() { let _ = one_or_two!(1); }",
        );
        assert!(output.contains("1"), "{output}");
    }
}
