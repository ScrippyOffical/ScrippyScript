//! Recursive descent parser with precedence climbing for binary operators.
//!
//! Statements are separated by newlines or `;`. Newlines are skipped where an
//! expression obviously continues (after an operator, comma or opening bracket)
//! so multi-line lists and calls work without trailing-comma tricks.

use crate::ast::{
    AssignOp, BinOp, BindKind, Block, Expr, MapEntry, Param, Script, Stmt, StrLit, StrPart,
    TypeRef, UnOp,
};
use crate::error::{ParseError, ParseErrorKind};
use crate::lexer::{tokenize, Keyword, Punct, Token, TokenKind};
use crate::span::Span;

/// Turns a token stream into a [`Script`]. Lex the source with
/// [`crate::lexer::tokenize`] first, or call [`crate::parse`] for both steps.
pub fn parse(tokens: Vec<Token<'_>>) -> Result<Script, ParseError> {
    Parser::new(tokens).parse_script()
}

/// Recursion cap for nested expressions and blocks. Deeper input gets a normal
/// error instead of overflowing the stack, which would abort the process on wasm.
const MAX_DEPTH: usize = 200;

struct Parser<'src> {
    toks: Vec<Token<'src>>,
    pos: usize,
    depth: usize,
}

impl<'src> Parser<'src> {
    fn new(toks: Vec<Token<'src>>) -> Self {
        Self {
            toks,
            pos: 0,
            depth: 0,
        }
    }

    /// Runs one level of descent, failing cleanly when the recursion limit is hit.
    fn guard<T>(&mut self, parse: fn(&mut Self) -> Result<T, ParseError>) -> Result<T, ParseError> {
        if self.depth >= MAX_DEPTH {
            return Err(ParseError::new(self.span(), ParseErrorKind::TooDeep));
        }
        self.depth += 1;
        let result = parse(self);
        self.depth -= 1;
        result
    }

    fn parse_script(&mut self) -> Result<Script, ParseError> {
        let stmts = self.parse_stmts()?;
        self.skip_separators();
        if !self.at_eof() {
            return self.expected("end of file");
        }
        Ok(Script { stmts })
    }

    fn parse_stmts(&mut self) -> Result<Vec<Stmt>, ParseError> {
        let mut stmts = Vec::new();
        loop {
            self.skip_separators();
            match self.peek() {
                None | Some(TokenKind::Punct(Punct::RBrace)) => break,
                _ => {}
            }
            stmts.push(self.parse_stmt()?);
            self.expect_separator()?;
        }
        Ok(stmts)
    }

    fn parse_stmt(&mut self) -> Result<Stmt, ParseError> {
        self.guard(Self::parse_stmt_inner)
    }

    fn parse_stmt_inner(&mut self) -> Result<Stmt, ParseError> {
        match self.peek() {
            Some(TokenKind::Keyword(Keyword::Let | Keyword::Const | Keyword::Global)) => {
                self.parse_bind()
            }
            Some(TokenKind::Keyword(Keyword::Fun))
                if matches!(self.peek_n(1), Some(TokenKind::Ident(_))) =>
            {
                self.parse_fn_decl()
            }
            Some(TokenKind::Keyword(Keyword::Return)) => self.parse_return(),
            Some(TokenKind::Keyword(Keyword::If)) => self.parse_if(),
            Some(TokenKind::Keyword(Keyword::While)) => self.parse_while(),
            Some(TokenKind::Keyword(Keyword::For)) => self.parse_for(),
            Some(TokenKind::Keyword(Keyword::Break)) => {
                let span = self.span();
                self.pos += 1;
                Ok(Stmt::Break { span })
            }
            Some(TokenKind::Keyword(Keyword::Continue)) => {
                let span = self.span();
                self.pos += 1;
                Ok(Stmt::Continue { span })
            }
            Some(TokenKind::Punct(Punct::LBrace)) => Ok(Stmt::Block(self.parse_block()?)),
            Some(TokenKind::Keyword(kw)) => match unsupported_feature(kw) {
                Some(feature) => Err(ParseError::unsupported(self.span(), feature)),
                None => self.parse_expr_or_assign(),
            },
            _ => self.parse_expr_or_assign(),
        }
    }

    fn parse_bind(&mut self) -> Result<Stmt, ParseError> {
        let start = self.span();
        let kind = match self.peek() {
            Some(TokenKind::Keyword(Keyword::Let)) => BindKind::Let,
            Some(TokenKind::Keyword(Keyword::Const)) => BindKind::Const,
            Some(TokenKind::Keyword(Keyword::Global)) => BindKind::Global,
            _ => return self.expected("a binding keyword"),
        };
        self.pos += 1;

        let (name, _) = self.expect_ident("a name")?;
        let ty = if self.eat_punct(Punct::Colon).is_some() {
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect_punct(Punct::Eq)?;
        let value = self.parse_expr()?;
        let span = start.to(value.span());
        Ok(Stmt::Bind {
            kind,
            name: name.to_string(),
            ty,
            value,
            span,
        })
    }

    fn parse_type(&mut self) -> Result<TypeRef, ParseError> {
        let (name, start) = self.expect_ident("a type name")?;
        let mut optional = false;
        let mut end = start;
        if let Some(span) = self.eat_punct(Punct::Question) {
            optional = true;
            end = span;
        }
        Ok(TypeRef {
            name: name.to_string(),
            optional,
            span: start.to(end),
        })
    }

    fn parse_fn_decl(&mut self) -> Result<Stmt, ParseError> {
        let start = self.span();
        self.pos += 1;
        let (name, _) = self.expect_ident("a function name")?;
        let params = self.parse_params()?;
        let body = self.parse_block()?;
        let span = start.to(body.span);
        Ok(Stmt::Fn {
            name: name.to_string(),
            params,
            body,
            span,
        })
    }

    fn parse_params(&mut self) -> Result<Vec<Param>, ParseError> {
        self.expect_punct(Punct::LParen)?;
        self.skip_newlines();
        let mut params = Vec::new();
        while !self.at_punct(Punct::RParen) {
            params.push(self.parse_param()?);
            self.skip_newlines();
            if self.eat_punct(Punct::Comma).is_none() {
                break;
            }
            self.skip_newlines();
        }
        self.expect_punct(Punct::RParen)?;
        Ok(params)
    }

    fn parse_param(&mut self) -> Result<Param, ParseError> {
        let (name, start) = self.expect_ident("a parameter name")?;
        let ty = if self.eat_punct(Punct::Colon).is_some() {
            Some(self.parse_type()?)
        } else {
            None
        };
        let default = if self.eat_punct(Punct::Eq).is_some() {
            Some(self.parse_expr()?)
        } else {
            None
        };
        let span = start.to(self.prev_span());
        Ok(Param {
            name: name.to_string(),
            ty,
            default,
            span,
        })
    }

    fn parse_return(&mut self) -> Result<Stmt, ParseError> {
        let start = self.span();
        self.pos += 1;
        let value = if self.at_stmt_end() {
            None
        } else {
            Some(self.parse_expr()?)
        };
        let span = start.to(value.as_ref().map_or(start, Expr::span));
        Ok(Stmt::Return { value, span })
    }

    fn parse_if(&mut self) -> Result<Stmt, ParseError> {
        let start = self.span();
        self.pos += 1;
        let cond = self.parse_expr()?;
        let then_branch = self.parse_block()?;
        let else_branch = self.parse_else()?;
        let span = start.to(self.prev_span());
        Ok(Stmt::If {
            cond,
            then_branch,
            else_branch,
            span,
        })
    }

    /// `else` may start on the line after the closing brace, so newlines are
    /// peeked at but only consumed when an `else` really is next.
    fn parse_else(&mut self) -> Result<Option<Box<Stmt>>, ParseError> {
        let save = self.pos;
        self.skip_separators();
        if !self.at_kw(Keyword::Else) {
            self.pos = save;
            return Ok(None);
        }
        self.pos += 1;

        if self.at_kw(Keyword::If) {
            Ok(Some(Box::new(self.parse_if()?)))
        } else {
            let block = self.parse_block()?;
            Ok(Some(Box::new(Stmt::Block(block))))
        }
    }

    fn parse_while(&mut self) -> Result<Stmt, ParseError> {
        let start = self.span();
        self.pos += 1;
        let cond = self.parse_expr()?;
        let body = self.parse_block()?;
        let span = start.to(body.span);
        Ok(Stmt::While { cond, body, span })
    }

    fn parse_for(&mut self) -> Result<Stmt, ParseError> {
        let start = self.span();
        self.pos += 1;
        let (var, _) = self.expect_ident("a loop variable")?;
        self.expect_kw(Keyword::In)?;
        let iter = self.parse_expr()?;
        let body = self.parse_block()?;
        let span = start.to(body.span);
        Ok(Stmt::For {
            var: var.to_string(),
            iter,
            body,
            span,
        })
    }

    fn parse_block(&mut self) -> Result<Block, ParseError> {
        // people write the brace on either line
        self.skip_newlines();
        let open = self.expect_punct(Punct::LBrace)?;
        let stmts = self.parse_stmts()?;
        let close = self.expect_punct(Punct::RBrace)?;
        Ok(Block {
            stmts,
            span: open.to(close),
        })
    }

    fn parse_expr_or_assign(&mut self) -> Result<Stmt, ParseError> {
        let target = self.parse_expr()?;
        let Some(op) = self.peek().and_then(assign_op) else {
            return Ok(Stmt::Expr(target));
        };
        self.pos += 1;
        self.skip_newlines();
        let value = self.parse_expr()?;

        if !is_assignable(&target) {
            return Err(ParseError::invalid_target(target.span()));
        }

        let span = target.span().to(value.span());
        Ok(Stmt::Assign {
            target,
            op,
            value,
            span,
        })
    }

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.guard(Self::parse_ternary)
    }

    fn parse_ternary(&mut self) -> Result<Expr, ParseError> {
        let cond = self.parse_binary(0)?;
        if self.eat_punct(Punct::Question).is_none() {
            return Ok(cond);
        }

        self.skip_newlines();
        let then_expr = self.parse_expr()?;
        self.skip_newlines();
        self.expect_punct(Punct::Colon)?;
        self.skip_newlines();
        let else_expr = self.parse_expr()?;
        let span = cond.span().to(else_expr.span());
        Ok(Expr::Ternary {
            cond: Box::new(cond),
            then_expr: Box::new(then_expr),
            else_expr: Box::new(else_expr),
            span,
        })
    }

    /// Precedence climbing. Higher numbers bind tighter.
    fn parse_binary(&mut self, min_prec: u8) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_unary()?;
        while let Some((op, prec, right_assoc)) = self.peek_binary_op() {
            if prec < min_prec {
                break;
            }
            self.pos += 1;
            self.skip_newlines();

            let next_min = if right_assoc { prec } else { prec + 1 };
            let rhs = self.parse_binary(next_min)?;
            let span = lhs.span().to(rhs.span());
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span,
            };
        }
        Ok(lhs)
    }

    fn peek_binary_op(&self) -> Option<(BinOp, u8, bool)> {
        Some(match self.peek()? {
            TokenKind::Punct(Punct::QuestionQuestion) => (BinOp::Coalesce, 2, true),
            TokenKind::Punct(Punct::OrOr) | TokenKind::Keyword(Keyword::Or) => {
                (BinOp::Or, 3, false)
            }
            TokenKind::Punct(Punct::AndAnd) | TokenKind::Keyword(Keyword::And) => {
                (BinOp::And, 4, false)
            }
            TokenKind::Punct(Punct::EqEq) => (BinOp::Eq, 5, false),
            TokenKind::Punct(Punct::BangEq) => (BinOp::Ne, 5, false),
            TokenKind::Punct(Punct::Lt) => (BinOp::Lt, 6, false),
            TokenKind::Punct(Punct::LtEq) => (BinOp::Le, 6, false),
            TokenKind::Punct(Punct::Gt) => (BinOp::Gt, 6, false),
            TokenKind::Punct(Punct::GtEq) => (BinOp::Ge, 6, false),
            TokenKind::Punct(Punct::DotDot) => (BinOp::Range, 7, false),
            TokenKind::Punct(Punct::DotDotLt) => (BinOp::RangeExcl, 7, false),
            TokenKind::Punct(Punct::Plus) => (BinOp::Add, 8, false),
            TokenKind::Punct(Punct::Minus) => (BinOp::Sub, 8, false),
            TokenKind::Punct(Punct::Star) => (BinOp::Mul, 9, false),
            TokenKind::Punct(Punct::Slash) => (BinOp::Div, 9, false),
            TokenKind::Punct(Punct::Percent) => (BinOp::Rem, 9, false),
            _ => return None,
        })
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        self.guard(Self::parse_unary_inner)
    }

    fn parse_unary_inner(&mut self) -> Result<Expr, ParseError> {
        let op = match self.peek() {
            Some(TokenKind::Punct(Punct::Minus)) => Some(UnOp::Neg),
            Some(TokenKind::Punct(Punct::Bang)) | Some(TokenKind::Keyword(Keyword::Not)) => {
                Some(UnOp::Not)
            }
            _ => None,
        };
        if let Some(op) = op {
            let start = self.span();
            self.pos += 1;
            self.skip_newlines();
            let expr = self.parse_unary()?;
            let span = start.to(expr.span());
            return Ok(Expr::Unary {
                op,
                expr: Box::new(expr),
                span,
            });
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_primary()?;
        loop {
            match self.peek() {
                Some(TokenKind::Punct(Punct::LParen)) => {
                    let (args, close) = self.parse_args()?;
                    let span = expr.span().to(close);
                    expr = Expr::Call {
                        callee: Box::new(expr),
                        args,
                        span,
                    };
                }
                Some(TokenKind::Punct(Punct::Dot) | TokenKind::Punct(Punct::QuestionDot)) => {
                    let safe = matches!(self.peek(), Some(TokenKind::Punct(Punct::QuestionDot)));
                    self.pos += 1;
                    let (name, name_span) = self.expect_name("a member name")?;
                    let span = expr.span().to(name_span);
                    expr = Expr::Member {
                        object: Box::new(expr),
                        name,
                        safe,
                        span,
                    };
                }
                Some(TokenKind::Punct(Punct::LBracket)) => {
                    self.pos += 1;
                    self.skip_newlines();
                    let index = self.parse_expr()?;
                    self.skip_newlines();
                    let close = self.expect_punct(Punct::RBracket)?;
                    let span = expr.span().to(close);
                    expr = Expr::Index {
                        object: Box::new(expr),
                        index: Box::new(index),
                        span,
                    };
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    fn parse_args(&mut self) -> Result<(Vec<Expr>, Span), ParseError> {
        self.expect_punct(Punct::LParen)?;
        self.skip_newlines();
        let mut args = Vec::new();
        while !self.at_punct(Punct::RParen) {
            args.push(self.parse_expr()?);
            self.skip_newlines();
            if self.eat_punct(Punct::Comma).is_none() {
                break;
            }
            self.skip_newlines();
        }
        let close = self.expect_punct(Punct::RParen)?;
        Ok((args, close))
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        let Some(tok) = self.peek() else {
            return self.expected("an expression");
        };

        match tok {
            TokenKind::Int(raw) => {
                let span = self.span();
                self.pos += 1;
                int_literal(raw, span)
            }
            TokenKind::Float(raw) => {
                let span = self.span();
                self.pos += 1;
                float_literal(raw, span)
            }
            TokenKind::Str(raw) => {
                let span = self.span();
                self.pos += 1;
                Ok(Expr::Str(self.string_literal(raw, span)?))
            }
            TokenKind::RawStr(raw) => {
                let span = self.span();
                self.pos += 1;
                let text = unescape(raw, span)?;
                let parts = if text.is_empty() {
                    Vec::new()
                } else {
                    vec![StrPart::Text(text)]
                };
                Ok(Expr::Str(StrLit { parts, span }))
            }
            TokenKind::Keyword(Keyword::True) => {
                let span = self.span();
                self.pos += 1;
                Ok(Expr::Bool { value: true, span })
            }
            TokenKind::Keyword(Keyword::False) => {
                let span = self.span();
                self.pos += 1;
                Ok(Expr::Bool { value: false, span })
            }
            TokenKind::Keyword(Keyword::Null) => {
                let span = self.span();
                self.pos += 1;
                Ok(Expr::Null { span })
            }
            TokenKind::Ident(name) => {
                let span = self.span();
                self.pos += 1;
                Ok(Expr::Ident {
                    name: name.to_string(),
                    span,
                })
            }
            TokenKind::Punct(Punct::LParen) => {
                self.pos += 1;
                self.skip_newlines();
                let expr = self.parse_expr()?;
                self.skip_newlines();
                self.expect_punct(Punct::RParen)?;
                Ok(expr)
            }
            TokenKind::Punct(Punct::LBracket) => self.parse_list(),
            TokenKind::Punct(Punct::LBrace) => self.parse_map(),
            TokenKind::Keyword(Keyword::Fun) => self.parse_func_expr(),
            TokenKind::Keyword(Keyword::New) => self.parse_new(),
            _ => self.expected("an expression"),
        }
    }

    fn parse_list(&mut self) -> Result<Expr, ParseError> {
        let start = self.span();
        self.pos += 1;
        self.skip_newlines();

        let mut items = Vec::new();
        while !self.at_punct(Punct::RBracket) {
            items.push(self.parse_expr()?);
            self.skip_newlines();
            if self.eat_punct(Punct::Comma).is_none() {
                break;
            }
            self.skip_newlines();
        }

        let close = self.expect_punct(Punct::RBracket)?;
        Ok(Expr::List {
            items,
            span: start.to(close),
        })
    }

    fn parse_map(&mut self) -> Result<Expr, ParseError> {
        let start = self.span();
        self.pos += 1;
        self.skip_newlines();

        let mut entries = Vec::new();
        while !self.at_punct(Punct::RBrace) {
            let (key, key_span) = match self.peek() {
                Some(TokenKind::Ident(_)) | Some(TokenKind::Keyword(_)) => {
                    self.expect_name("a map key")?
                }
                Some(TokenKind::Str(raw)) => {
                    let span = self.span();
                    self.pos += 1;
                    (self.text_key(raw, span)?, span)
                }
                _ => return self.expected("a map key"),
            };

            self.expect_punct(Punct::Colon)?;
            self.skip_newlines();
            let value = self.parse_expr()?;
            let span = key_span.to(value.span());
            entries.push(MapEntry { key, value, span });

            self.skip_newlines();
            if self.eat_punct(Punct::Comma).is_none() {
                break;
            }
            self.skip_newlines();
        }

        let close = self.expect_punct(Punct::RBrace)?;
        Ok(Expr::Map {
            entries,
            span: start.to(close),
        })
    }

    fn parse_func_expr(&mut self) -> Result<Expr, ParseError> {
        let start = self.span();
        self.pos += 1;
        let params = self.parse_params()?;
        let body = self.parse_block()?;
        let span = start.to(body.span);
        Ok(Expr::Func { params, body, span })
    }

    fn parse_new(&mut self) -> Result<Expr, ParseError> {
        let start = self.span();
        self.pos += 1;
        let (name, _) = self.expect_ident("a type name")?;
        let (args, close) = self.parse_args()?;
        Ok(Expr::New {
            name: name.to_string(),
            args,
            span: start.to(close),
        })
    }

    /// Splits a double-quoted string into text and `${...}` expression parts.
    fn string_literal(&self, raw: &str, span: Span) -> Result<StrLit, ParseError> {
        // +1 skips the opening quote so fragment offsets land in the source.
        let base = span.start + 1;
        let mut parts = Vec::new();
        let mut text = String::new();
        let mut i = 0;

        while let Some(c) = char_at(raw, i) {
            match c {
                '\\' => {
                    let (decoded, next) = decode_escape(raw, i, base)?;
                    text.push(decoded);
                    i = next;
                }
                '$' if raw[i + 1..].starts_with('{') => {
                    let expr_start = i + 2;
                    let Some(end) = interpolation_end(raw, expr_start) else {
                        return Err(ParseError::new(
                            span,
                            ParseErrorKind::UnterminatedInterpolation,
                        ));
                    };
                    if end == expr_start {
                        return Err(ParseError::new(
                            Span::new(base + expr_start, base + expr_start),
                            ParseErrorKind::BadInterpolation("`${}` is empty".into()),
                        ));
                    }
                    if !text.is_empty() {
                        parts.push(StrPart::Text(std::mem::take(&mut text)));
                    }
                    let expr = self.parse_fragment(&raw[expr_start..end], base + expr_start)?;
                    parts.push(StrPart::Expr(Box::new(expr)));
                    i = end + 1;
                }
                _ => {
                    text.push(c);
                    i += c.len_utf8();
                }
            }
        }

        if !text.is_empty() {
            parts.push(StrPart::Text(text));
        }
        Ok(StrLit { parts, span })
    }

    /// Map keys have to be static text, so interpolation is rejected instead of
    /// being silently kept as a literal `${...}`.
    fn text_key(&self, raw: &str, span: Span) -> Result<String, ParseError> {
        let lit = self.string_literal(raw, span)?;
        let mut key = String::new();
        for part in lit.parts {
            match part {
                StrPart::Text(text) => key.push_str(&text),
                StrPart::Expr(_) => {
                    return Err(ParseError::new(span, ParseErrorKind::BadMapKey));
                }
            }
        }
        Ok(key)
    }

    /// Lexes and parses the inside of a `${...}` chunk. Spans are shifted back
    /// into the enclosing file so diagnostics point at the real location.
    fn parse_fragment(&self, text: &str, base: usize) -> Result<Expr, ParseError> {
        let mut toks = tokenize(text).map_err(|errors| {
            // tokenize only returns Err after collecting at least one error
            let err = errors
                .first()
                .expect("tokenize never returns an empty list");
            ParseError::new(
                err.span.offset(base),
                ParseErrorKind::BadInterpolation(err.kind.to_string()),
            )
        })?;
        for tok in &mut toks {
            tok.span = tok.span.offset(base);
        }

        let mut sub = Parser::new(toks);
        sub.skip_newlines();
        let expr = sub.parse_expr()?;
        sub.skip_newlines();
        if !sub.at_eof() {
            return sub.expected("end of interpolation");
        }
        Ok(expr)
    }

    fn peek(&self) -> Option<TokenKind<'src>> {
        self.toks.get(self.pos).map(|t| t.kind)
    }

    fn peek_n(&self, n: usize) -> Option<TokenKind<'src>> {
        self.toks.get(self.pos + n).map(|t| t.kind)
    }

    fn at_eof(&self) -> bool {
        self.pos >= self.toks.len()
    }

    fn at_punct(&self, punct: Punct) -> bool {
        matches!(self.peek(), Some(TokenKind::Punct(p)) if p == punct)
    }

    fn at_kw(&self, kw: Keyword) -> bool {
        matches!(self.peek(), Some(TokenKind::Keyword(k)) if k == kw)
    }

    fn at_stmt_end(&self) -> bool {
        matches!(
            self.peek(),
            None | Some(TokenKind::Newline)
                | Some(TokenKind::Punct(Punct::Semi))
                | Some(TokenKind::Punct(Punct::RBrace))
        )
    }

    fn eat_punct(&mut self, punct: Punct) -> Option<Span> {
        if !self.at_punct(punct) {
            return None;
        }
        let span = self.span();
        self.pos += 1;
        Some(span)
    }

    fn expect_punct(&mut self, punct: Punct) -> Result<Span, ParseError> {
        if let Some(span) = self.eat_punct(punct) {
            return Ok(span);
        }
        self.expected(format!("`{}`", punct.as_str()))
    }

    fn expect_kw(&mut self, kw: Keyword) -> Result<Span, ParseError> {
        if self.at_kw(kw) {
            let span = self.span();
            self.pos += 1;
            return Ok(span);
        }
        self.expected(format!("`{}`", kw.as_str()))
    }

    fn expect_ident(&mut self, what: &str) -> Result<(&'src str, Span), ParseError> {
        match self.peek() {
            Some(TokenKind::Ident(name)) => {
                let span = self.span();
                self.pos += 1;
                Ok((name, span))
            }
            _ => self.expected(what),
        }
    }

    /// Member names and map keys may collide with keywords (`x.default`,
    /// `{ match: 1 }`), because the host API is reached by name.
    fn expect_name(&mut self, what: &str) -> Result<(String, Span), ParseError> {
        let name = match self.peek() {
            Some(TokenKind::Ident(name)) => name.to_string(),
            Some(TokenKind::Keyword(kw)) => kw.as_str().to_string(),
            _ => return self.expected(what),
        };
        let span = self.span();
        self.pos += 1;
        Ok((name, span))
    }

    fn span(&self) -> Span {
        self.toks
            .get(self.pos)
            .map_or_else(|| self.eof_span(), |tok| tok.span)
    }

    fn prev_span(&self) -> Span {
        self.toks
            .get(self.pos.saturating_sub(1))
            .map_or(Span::default(), |tok| tok.span)
    }

    fn eof_span(&self) -> Span {
        let end = self.toks.last().map_or(0, |tok| tok.span.end);
        Span::new(end, end)
    }

    fn expected<T>(&self, what: impl Into<String>) -> Result<T, ParseError> {
        Err(self.err_expected(what.into()))
    }

    fn err_expected(&self, what: String) -> ParseError {
        match self.toks.get(self.pos) {
            Some(tok) => ParseError::unexpected(tok.span, tok.kind.describe(), what),
            None => ParseError::eof(self.eof_span(), what),
        }
    }

    fn skip_newlines(&mut self) {
        while matches!(self.peek(), Some(TokenKind::Newline)) {
            self.pos += 1;
        }
    }

    fn skip_separators(&mut self) {
        while matches!(
            self.peek(),
            Some(TokenKind::Newline) | Some(TokenKind::Punct(Punct::Semi))
        ) {
            self.pos += 1;
        }
    }

    /// Statements need a newline or `;` between them, unless the next token is
    /// the end of the block.
    fn expect_separator(&mut self) -> Result<(), ParseError> {
        match self.peek() {
            None | Some(TokenKind::Punct(Punct::RBrace)) => Ok(()),
            Some(TokenKind::Newline) | Some(TokenKind::Punct(Punct::Semi)) => {
                self.skip_separators();
                Ok(())
            }
            _ => self.expected("a newline or `;`"),
        }
    }
}

fn assign_op(tok: TokenKind<'_>) -> Option<AssignOp> {
    Some(match tok {
        TokenKind::Punct(Punct::Eq) => AssignOp::Assign,
        TokenKind::Punct(Punct::PlusEq) => AssignOp::Add,
        TokenKind::Punct(Punct::MinusEq) => AssignOp::Sub,
        TokenKind::Punct(Punct::StarEq) => AssignOp::Mul,
        TokenKind::Punct(Punct::SlashEq) => AssignOp::Div,
        TokenKind::Punct(Punct::PercentEq) => AssignOp::Rem,
        _ => return None,
    })
}

fn is_assignable(expr: &Expr) -> bool {
    match expr {
        Expr::Ident { .. } | Expr::Index { .. } => true,
        Expr::Member { safe, .. } => !safe,
        _ => false,
    }
}

/// Keywords that are valid Scrippy but not implemented here. Reporting them by
/// name beats a generic "expected an expression" at the start of a block.
fn unsupported_feature(kw: Keyword) -> Option<&'static str> {
    Some(match kw {
        Keyword::On => "event handlers aren't supported yet",
        Keyword::Command => "command declarations aren't supported yet",
        Keyword::Every | Keyword::After => "scheduling blocks aren't supported yet",
        Keyword::Import => "imports aren't supported yet",
        Keyword::Match | Keyword::Case | Keyword::Default => "match blocks aren't supported yet",
        Keyword::Try | Keyword::Catch | Keyword::Finally | Keyword::Throw => {
            "error handling isn't supported yet"
        }
        _ => return None,
    })
}

fn int_literal(raw: &str, span: Span) -> Result<Expr, ParseError> {
    let digits = raw.replace('_', "");
    let parsed = match digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
    {
        Some(hex) => i64::from_str_radix(hex, 16),
        None => digits.parse::<i64>(),
    };
    parsed
        .map(|value| Expr::Int { value, span })
        .map_err(|_| ParseError::bad_number(span, raw))
}

fn float_literal(raw: &str, span: Span) -> Result<Expr, ParseError> {
    raw.replace('_', "")
        .parse::<f64>()
        .map(|value| Expr::Float { value, span })
        .map_err(|_| ParseError::bad_number(span, raw))
}

/// Decodes the escape at `raw[index]` (a backslash) and returns the character
/// plus the index just past it. `base` maps raw offsets back to the source.
fn decode_escape(raw: &str, index: usize, base: usize) -> Result<(char, usize), ParseError> {
    let Some(escaped) = char_at(raw, index + 1) else {
        return Err(ParseError::bad_escape(
            Span::new(base + index, base + index + 1),
            '\\',
        ));
    };

    let decoded = unescape_char(escaped).ok_or_else(|| {
        ParseError::bad_escape(
            Span::new(base + index + 1, base + index + 1 + escaped.len_utf8()),
            escaped,
        )
    })?;
    Ok((decoded, index + 1 + escaped.len_utf8()))
}

/// Expands escapes in text that is not split on `${...}`.
fn unescape(raw: &str, span: Span) -> Result<String, ParseError> {
    let base = span.start + 1;
    let mut out = String::with_capacity(raw.len());
    let mut i = 0;

    while let Some(c) = char_at(raw, i) {
        if c == '\\' {
            let (decoded, next) = decode_escape(raw, i, base)?;
            out.push(decoded);
            i = next;
        } else {
            out.push(c);
            i += c.len_utf8();
        }
    }
    Ok(out)
}

fn unescape_char(c: char) -> Option<char> {
    Some(match c {
        'n' => '\n',
        'r' => '\r',
        't' => '\t',
        '0' => '\0',
        '\\' => '\\',
        '\'' => '\'',
        '"' => '"',
        '$' => '$',
        '{' => '{',
        '}' => '}',
        _ => return None,
    })
}

fn char_at(text: &str, index: usize) -> Option<char> {
    text.get(index..)?.chars().next()
}

/// Returns the index of the `}` that closes the interpolation starting at
/// `start`, skipping braces inside nested maps and string literals.
fn interpolation_end(raw: &str, start: usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut i = start;

    while let Some(c) = char_at(raw, i) {
        match c {
            '\\' => {
                i += 1;
                if let Some(next) = char_at(raw, i) {
                    i += next.len_utf8();
                }
            }
            '\'' | '"' => i = skip_quoted(raw, i)?,
            '{' => {
                depth += 1;
                i += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
                i += 1;
            }
            _ => i += c.len_utf8(),
        }
    }
    None
}

/// Index just past the closing quote, or `None` for an unterminated literal.
fn skip_quoted(raw: &str, start: usize) -> Option<usize> {
    let quote = char_at(raw, start)?;
    let mut i = start + 1;

    while let Some(c) = char_at(raw, i) {
        match c {
            '\\' => {
                i += 1;
                if let Some(next) = char_at(raw, i) {
                    i += next.len_utf8();
                }
            }
            c if c == quote => return Some(i + 1),
            '\n' => return None,
            _ => i += c.len_utf8(),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_src(src: &str) -> Script {
        crate::parse(src).unwrap_or_else(|errs| panic!("{src:?} failed: {errs:?}"))
    }

    fn parse_err(src: &str) -> Vec<crate::Error> {
        crate::parse(src).unwrap_err()
    }

    #[test]
    fn parses_let_and_const() {
        let script = parse_src("let x = 1\nconst name = \"hi\"\nglobal visits: number = 0");
        assert_eq!(script.stmts.len(), 3);

        let Stmt::Bind {
            kind, name, value, ..
        } = &script.stmts[0]
        else {
            panic!("expected a binding");
        };
        assert_eq!(*kind, BindKind::Let);
        assert_eq!(name, "x");
        assert!(matches!(value, Expr::Int { value: 1, .. }));

        let Stmt::Bind {
            kind, ty: Some(ty), ..
        } = &script.stmts[2]
        else {
            panic!("expected a typed binding");
        };
        assert_eq!(*kind, BindKind::Global);
        assert_eq!(ty.name, "number");
    }

    #[test]
    fn parses_functions() {
        let script =
            parse_src("fun greet(name, greeting: string = \"Hello\") {\n  return greeting\n}");
        let Stmt::Fn { name, params, .. } = &script.stmts[0] else {
            panic!("expected a function");
        };
        assert_eq!(name, "greet");
        assert_eq!(params.len(), 2);
        assert_eq!(params[1].name, "greeting");
        assert_eq!(params[1].ty.as_ref().unwrap().name, "string");
        assert!(params[1].default.is_some());
    }

    #[test]
    fn precedence_follows_math() {
        let script = parse_src("let x = 1 + 2 * 3");
        let Stmt::Bind { value, .. } = &script.stmts[0] else {
            panic!();
        };
        let Expr::Binary { op, rhs, .. } = value else {
            panic!("expected a binary expression, got {value:?}");
        };
        assert_eq!(*op, BinOp::Add);
        assert!(matches!(**rhs, Expr::Binary { op: BinOp::Mul, .. }));
    }

    #[test]
    fn calls_and_members_chain() {
        let script = parse_src("ScrippyAI.ask(\"hi\")\nplayer.inventory.addItem(sword)");
        let Stmt::Expr(Expr::Call { callee, args, .. }) = &script.stmts[0] else {
            panic!("expected a call");
        };
        assert!(matches!(&**callee, Expr::Member { name, .. } if name == "ask"));
        assert_eq!(args.len(), 1);

        let Stmt::Expr(Expr::Call { callee, .. }) = &script.stmts[1] else {
            panic!("expected a call");
        };
        assert!(matches!(&**callee, Expr::Member { name, .. } if name == "addItem"));
    }

    #[test]
    fn string_interpolation_becomes_an_expression() {
        let script = parse_src(r#"let msg = "hp: ${player.health}""#);
        let Stmt::Bind {
            value: Expr::Str(lit),
            ..
        } = &script.stmts[0]
        else {
            panic!("expected a string");
        };
        assert_eq!(lit.parts.len(), 2);

        let StrPart::Text(text) = &lit.parts[0] else {
            panic!("expected text");
        };
        assert_eq!(text, "hp: ");

        let StrPart::Expr(expr) = &lit.parts[1] else {
            panic!("expected an interpolation");
        };
        assert!(matches!(&**expr, Expr::Member { name, .. } if name == "health"));
    }

    #[test]
    fn single_quoted_strings_stay_literal() {
        let script = parse_src("let path = '${not.interpolated}'");
        let Stmt::Bind {
            value: Expr::Str(lit),
            ..
        } = &script.stmts[0]
        else {
            panic!();
        };
        assert_eq!(lit.parts, vec![StrPart::Text("${not.interpolated}".into())]);
    }

    #[test]
    fn interpolation_accepts_single_quoted_arguments() {
        let script = parse_src(r#"let msg = "a ${join('x')} b""#);
        let Stmt::Bind {
            value: Expr::Str(lit),
            ..
        } = &script.stmts[0]
        else {
            panic!("expected a string");
        };

        assert!(matches!(&lit.parts[0], StrPart::Text(text) if text == "a "));
        assert!(matches!(&lit.parts[2], StrPart::Text(text) if text == " b"));

        let StrPart::Expr(expr) = &lit.parts[1] else {
            panic!("expected an interpolation");
        };
        let Expr::Call { callee, args, .. } = &**expr else {
            panic!("expected a call, got {expr:?}");
        };
        assert!(matches!(&**callee, Expr::Ident { name, .. } if name == "join"));

        let Expr::Str(argument) = &args[0] else {
            panic!("expected a string argument");
        };
        assert!(matches!(&argument.parts[..], [StrPart::Text(text)] if text == "x"));
    }

    #[test]
    fn parses_ternary_and_coalesce() {
        let script = parse_src("let who = target != null ? target : sender\nlet n = a ?? b ?? 0");
        assert!(matches!(
            &script.stmts[0],
            Stmt::Bind {
                value: Expr::Ternary { .. },
                ..
            }
        ));
        assert!(matches!(
            &script.stmts[1],
            Stmt::Bind {
                value: Expr::Binary {
                    op: BinOp::Coalesce,
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn parses_ranges_and_aliases() {
        let script =
            parse_src("for i in 0..<10 {\n  if not done and i >= 2 {\n    print(i)\n  }\n}");
        let Stmt::For { iter, body, .. } = &script.stmts[0] else {
            panic!("expected a for loop");
        };
        assert!(matches!(
            iter,
            Expr::Binary {
                op: BinOp::RangeExcl,
                ..
            }
        ));

        let Stmt::If { cond, .. } = &body.stmts[0] else {
            panic!("expected an if");
        };
        assert!(matches!(cond, Expr::Binary { op: BinOp::And, .. }));
    }

    #[test]
    fn parses_else_if_chain() {
        let script = parse_src(
            "if x > 5 {\n  print(\"big\")\n} else if x == 5 {\n  print(\"exact\")\n} else {\n  print(\"small\")\n}",
        );
        let Stmt::If { else_branch, .. } = &script.stmts[0] else {
            panic!("expected an if");
        };
        let Some(else_branch) = else_branch else {
            panic!("expected an else branch");
        };
        let Stmt::If { else_branch, .. } = &**else_branch else {
            panic!("expected else if");
        };
        assert!(matches!(**else_branch.as_ref().unwrap(), Stmt::Block(_)));
    }

    #[test]
    fn parses_assignments() {
        let script = parse_src("x = 1\nx += 5\nplayer.health = 20\nscores[\"alice\"] = 5");

        assert!(matches!(
            &script.stmts[0],
            Stmt::Assign {
                op: AssignOp::Assign,
                ..
            }
        ));
        assert!(matches!(
            &script.stmts[1],
            Stmt::Assign {
                op: AssignOp::Add,
                ..
            }
        ));
        assert!(matches!(
            &script.stmts[2],
            Stmt::Assign {
                target: Expr::Member { safe: false, .. },
                ..
            }
        ));
        assert!(matches!(
            &script.stmts[3],
            Stmt::Assign {
                target: Expr::Index { .. },
                ..
            }
        ));
    }

    #[test]
    fn parses_lists_and_maps() {
        let script =
            parse_src("let items = [1, 2, 3,]\nlet player = { name: \"Steve\", level: 3 }");
        let Stmt::Bind {
            value: Expr::List { items, .. },
            ..
        } = &script.stmts[0]
        else {
            panic!("expected a list");
        };
        assert_eq!(items.len(), 3);

        let Stmt::Bind {
            value: Expr::Map { entries, .. },
            ..
        } = &script.stmts[1]
        else {
            panic!("expected a map");
        };
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].key, "name");
    }

    #[test]
    fn parses_new_and_safe_access() {
        let script =
            parse_src("let stack = new ItemStack(material(\"STONE\"), 1)\nlet n = a?.b?.c");
        let Stmt::Bind {
            value: Expr::New { name, args, .. },
            ..
        } = &script.stmts[0]
        else {
            panic!("expected a constructor call");
        };
        assert_eq!(name, "ItemStack");
        assert_eq!(args.len(), 2);

        let Stmt::Bind { value, .. } = &script.stmts[1] else {
            panic!();
        };
        assert!(matches!(value, Expr::Member { safe: true, .. }));
    }

    #[test]
    fn parses_hex_and_underscored_numbers() {
        let script = parse_src("let a = 0xFF\nlet b = 1_000\nlet c = 2e3");
        assert!(matches!(
            &script.stmts[0],
            Stmt::Bind {
                value: Expr::Int { value: 255, .. },
                ..
            }
        ));
        assert!(matches!(
            &script.stmts[1],
            Stmt::Bind {
                value: Expr::Int { value: 1000, .. },
                ..
            }
        ));
        assert!(
            matches!(&script.stmts[2], Stmt::Bind { value: Expr::Float { value, .. }, .. } if *value == 2000.0)
        );
    }

    #[test]
    fn semicolons_also_end_statements() {
        let script = parse_src("let x = 1; let y = 2 // done");
        assert_eq!(script.stmts.len(), 2);
    }

    #[test]
    fn assignment_target_must_be_storable() {
        let errors = parse_err("1 = 2");
        assert!(matches!(
            errors[0],
            crate::Error::Parse(ParseError {
                kind: ParseErrorKind::InvalidAssignTarget,
                ..
            })
        ));
    }

    #[test]
    fn missing_statement_separator_is_reported() {
        let errors = parse_err("let x = 1 let y = 2");
        let message = errors[0].to_string();
        assert!(message.contains("newline or `;`"), "{message}");
    }

    #[test]
    fn unterminated_string_is_a_lex_error() {
        let errors = parse_err("let x = \"oops");
        assert!(matches!(errors[0], crate::Error::Lex(_)));
    }

    #[test]
    fn event_blocks_get_a_helpful_message() {
        let errors = parse_err("on join {\n}");
        let message = errors[0].to_string();
        assert!(message.contains("event handlers"), "{message}");
    }

    #[test]
    fn interpolation_reports_the_expression_location() {
        let src = "let x = \"${1 + }\"";
        let errors = parse_err(src);
        let rendered = errors[0].render(src);
        assert!(rendered.contains("1:15"), "{rendered}");
    }

    #[test]
    fn empty_interpolation_is_rejected() {
        let errors = parse_err("let x = \"${}\"");
        let message = errors[0].to_string();
        assert!(message.contains("`${}` is empty"), "{message}");
    }

    #[test]
    fn deep_nesting_does_not_overflow_the_stack() {
        let nested_lists = format!("let x = {}", "[".repeat(500));
        let errors = parse_err(&nested_lists);
        assert!(errors[0].to_string().contains("nesting is too deep"));

        let nested_blocks = "{".repeat(500);
        let errors = parse_err(&nested_blocks);
        assert!(errors[0].to_string().contains("nesting is too deep"));

        let nested_nots = format!("let x = {}", "!".repeat(1000));
        let errors = parse_err(&nested_nots);
        assert!(errors[0].to_string().contains("nesting is too deep"));
    }

    #[test]
    fn braces_can_start_on_the_next_line() {
        let script = parse_src("if x > 5\n{\n  print(\"big\")\n}\nfun f()\n{\n  return 1\n}");
        assert_eq!(script.stmts.len(), 2);
    }

    #[test]
    fn interpolated_map_keys_are_rejected() {
        let errors = parse_err("let m = { \"${name}\": 1 }");
        assert!(errors[0].to_string().contains("map keys cannot"));

        // An escaped dollar is plain text, so this key is fine.
        let script = parse_src("let m = { \"\\${name}\": 1 }");
        let Stmt::Bind {
            value: Expr::Map { entries, .. },
            ..
        } = &script.stmts[0]
        else {
            panic!("expected a map");
        };
        assert_eq!(entries[0].key, "${name}");
    }

    #[test]
    fn ternary_can_break_around_the_question_mark() {
        let script = parse_src("let x = cond ?\n  1\n: 2");
        assert!(matches!(
            &script.stmts[0],
            Stmt::Bind {
                value: Expr::Ternary { .. },
                ..
            }
        ));
    }

    #[test]
    fn keywords_work_as_member_names_and_map_keys() {
        let script = parse_src("let a = player.default\nlet m = { match: 1, new: 2 }");
        assert!(matches!(
            &script.stmts[0],
            Stmt::Bind {
                value: Expr::Member { name, .. },
                ..
            } if name == "default"
        ));

        let Stmt::Bind {
            value: Expr::Map { entries, .. },
            ..
        } = &script.stmts[1]
        else {
            panic!("expected a map");
        };
        assert_eq!(entries[0].key, "match");
        assert_eq!(entries[1].key, "new");
    }

    #[test]
    fn incomplete_hex_literal_is_reported_as_a_number() {
        let errors = parse_err("let a = 0x");
        assert!(errors[0]
            .to_string()
            .contains("invalid number literal `0x`"));
    }

    #[test]
    fn unknown_escapes_point_at_the_escaped_character() {
        for src in ["let x = \"a \\q\"", "let x = 'a \\q'"] {
            let errors = parse_err(src);
            let rendered = errors[0].render(src);
            assert!(
                rendered.contains("unknown escape sequence `\\q`"),
                "{rendered}"
            );
            assert!(rendered.contains("1:13"), "{rendered}");
        }
    }
}
