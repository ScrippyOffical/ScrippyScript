//! Tokenizer for `.scp` source.
//!
//! `logos` turns the token patterns into a DFA at compile time, so tokens are
//! just slices of the input and there is nothing to pull in at runtime.

use logos::Logos;

use crate::error::{LexError, LexErrorKind};
use crate::span::Span;

/// Reserved words. `and`, `or` and `not` are aliases for `&&`, `||` and `!`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keyword {
    Let,
    Const,
    Global,
    Fun,
    Return,
    If,
    Else,
    While,
    For,
    In,
    Break,
    Continue,
    True,
    False,
    Null,
    And,
    Or,
    Not,
    New,
    Import,
    As,
    On,
    Every,
    After,
    Command,
    Match,
    Case,
    Default,
    Try,
    Catch,
    Finally,
    Throw,
}

impl Keyword {
    pub fn as_str(self) -> &'static str {
        match self {
            Keyword::Let => "let",
            Keyword::Const => "const",
            Keyword::Global => "global",
            Keyword::Fun => "fun",
            Keyword::Return => "return",
            Keyword::If => "if",
            Keyword::Else => "else",
            Keyword::While => "while",
            Keyword::For => "for",
            Keyword::In => "in",
            Keyword::Break => "break",
            Keyword::Continue => "continue",
            Keyword::True => "true",
            Keyword::False => "false",
            Keyword::Null => "null",
            Keyword::And => "and",
            Keyword::Or => "or",
            Keyword::Not => "not",
            Keyword::New => "new",
            Keyword::Import => "import",
            Keyword::As => "as",
            Keyword::On => "on",
            Keyword::Every => "every",
            Keyword::After => "after",
            Keyword::Command => "command",
            Keyword::Match => "match",
            Keyword::Case => "case",
            Keyword::Default => "default",
            Keyword::Try => "try",
            Keyword::Catch => "catch",
            Keyword::Finally => "finally",
            Keyword::Throw => "throw",
        }
    }
}

/// Punctuation and operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Punct {
    LBrace,
    RBrace,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    Colon,
    Semi,
    Dot,
    DotDot,
    DotDotLt,
    Question,
    QuestionDot,
    QuestionQuestion,
    Eq,
    EqEq,
    Bang,
    BangEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    AndAnd,
    OrOr,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
}

impl Punct {
    pub fn as_str(self) -> &'static str {
        match self {
            Punct::LBrace => "{",
            Punct::RBrace => "}",
            Punct::LParen => "(",
            Punct::RParen => ")",
            Punct::LBracket => "[",
            Punct::RBracket => "]",
            Punct::Comma => ",",
            Punct::Colon => ":",
            Punct::Semi => ";",
            Punct::Dot => ".",
            Punct::DotDot => "..",
            Punct::DotDotLt => "..<",
            Punct::Question => "?",
            Punct::QuestionDot => "?.",
            Punct::QuestionQuestion => "??",
            Punct::Eq => "=",
            Punct::EqEq => "==",
            Punct::Bang => "!",
            Punct::BangEq => "!=",
            Punct::Lt => "<",
            Punct::LtEq => "<=",
            Punct::Gt => ">",
            Punct::GtEq => ">=",
            Punct::AndAnd => "&&",
            Punct::OrOr => "||",
            Punct::Plus => "+",
            Punct::Minus => "-",
            Punct::Star => "*",
            Punct::Slash => "/",
            Punct::Percent => "%",
            Punct::PlusEq => "+=",
            Punct::MinusEq => "-=",
            Punct::StarEq => "*=",
            Punct::SlashEq => "/=",
            Punct::PercentEq => "%=",
        }
    }
}

#[derive(Logos, Debug, Clone, Copy, PartialEq, Eq)]
#[logos(skip r"[ \t\f\v]+")]
#[logos(skip(r"(//|#)[^\n]*", allow_greedy = true))]
#[logos(skip r"/\*([^*]|\*+[^*/])*\*+/")]
pub enum TokenKind<'src> {
    #[regex(r"\r\n|\r|\n")]
    Newline,

    #[token("let", |_| Keyword::Let)]
    #[token("const", |_| Keyword::Const)]
    #[token("global", |_| Keyword::Global)]
    #[token("fun", |_| Keyword::Fun)]
    #[token("return", |_| Keyword::Return)]
    #[token("if", |_| Keyword::If)]
    #[token("else", |_| Keyword::Else)]
    #[token("while", |_| Keyword::While)]
    #[token("for", |_| Keyword::For)]
    #[token("in", |_| Keyword::In)]
    #[token("break", |_| Keyword::Break)]
    #[token("continue", |_| Keyword::Continue)]
    #[token("true", |_| Keyword::True)]
    #[token("false", |_| Keyword::False)]
    #[token("null", |_| Keyword::Null)]
    #[token("and", |_| Keyword::And)]
    #[token("or", |_| Keyword::Or)]
    #[token("not", |_| Keyword::Not)]
    #[token("new", |_| Keyword::New)]
    #[token("import", |_| Keyword::Import)]
    #[token("as", |_| Keyword::As)]
    #[token("on", |_| Keyword::On)]
    #[token("every", |_| Keyword::Every)]
    #[token("after", |_| Keyword::After)]
    #[token("command", |_| Keyword::Command)]
    #[token("match", |_| Keyword::Match)]
    #[token("case", |_| Keyword::Case)]
    #[token("default", |_| Keyword::Default)]
    #[token("try", |_| Keyword::Try)]
    #[token("catch", |_| Keyword::Catch)]
    #[token("finally", |_| Keyword::Finally)]
    #[token("throw", |_| Keyword::Throw)]
    Keyword(Keyword),

    // The trailing `*` lets a bare `0x` reach the parser, which reports it as a
    // bad number instead of splitting it into `0` and an identifier.
    #[regex(r"0[xX][0-9a-fA-F_]*", |lex| lex.slice())]
    #[regex(r"[0-9][0-9_]*", |lex| lex.slice())]
    Int(&'src str),

    #[regex(r"[0-9][0-9_]*\.[0-9][0-9_]*([eE][+-]?[0-9]+)?", |lex| lex.slice())]
    #[regex(r"[0-9][0-9_]*[eE][+-]?[0-9]+", |lex| lex.slice())]
    Float(&'src str),

    /// Contents of a double-quoted string, without the quotes. `${...}` is
    /// resolved by the parser.
    #[regex(r#""([^"\\\r\n]|\\.)*""#, |lex| strip_quotes(lex.slice()))]
    Str(&'src str),

    /// Contents of a single-quoted string. Always literal.
    #[regex(r"'([^'\\\r\n]|\\.)*'", |lex| strip_quotes(lex.slice()))]
    RawStr(&'src str),

    #[regex(r"[A-Za-z_][A-Za-z0-9_]*", |lex| lex.slice())]
    Ident(&'src str),

    #[token("{", |_| Punct::LBrace)]
    #[token("}", |_| Punct::RBrace)]
    #[token("(", |_| Punct::LParen)]
    #[token(")", |_| Punct::RParen)]
    #[token("[", |_| Punct::LBracket)]
    #[token("]", |_| Punct::RBracket)]
    #[token(",", |_| Punct::Comma)]
    #[token(":", |_| Punct::Colon)]
    #[token(";", |_| Punct::Semi)]
    #[token("?.", |_| Punct::QuestionDot)]
    #[token("??", |_| Punct::QuestionQuestion)]
    #[token("?", |_| Punct::Question)]
    #[token("..<", |_| Punct::DotDotLt)]
    #[token("..", |_| Punct::DotDot)]
    #[token(".", |_| Punct::Dot)]
    #[token("==", |_| Punct::EqEq)]
    #[token("=", |_| Punct::Eq)]
    #[token("!=", |_| Punct::BangEq)]
    #[token("!", |_| Punct::Bang)]
    #[token("<=", |_| Punct::LtEq)]
    #[token("<", |_| Punct::Lt)]
    #[token(">=", |_| Punct::GtEq)]
    #[token(">", |_| Punct::Gt)]
    #[token("&&", |_| Punct::AndAnd)]
    #[token("||", |_| Punct::OrOr)]
    #[token("+=", |_| Punct::PlusEq)]
    #[token("+", |_| Punct::Plus)]
    #[token("-=", |_| Punct::MinusEq)]
    #[token("-", |_| Punct::Minus)]
    #[token("*=", |_| Punct::StarEq)]
    #[token("*", |_| Punct::Star)]
    #[token("/=", |_| Punct::SlashEq)]
    #[token("/", |_| Punct::Slash)]
    #[token("%=", |_| Punct::PercentEq)]
    #[token("%", |_| Punct::Percent)]
    Punct(Punct),
}

/// A token with its location in the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token<'src> {
    pub kind: TokenKind<'src>,
    pub span: Span,
}

impl TokenKind<'_> {
    /// Human readable name for error messages.
    pub fn describe(&self) -> String {
        match self {
            TokenKind::Newline => "newline".into(),
            TokenKind::Keyword(kw) => format!("`{}`", kw.as_str()),
            TokenKind::Int(_) => "integer".into(),
            TokenKind::Float(_) => "number".into(),
            TokenKind::Str(_) | TokenKind::RawStr(_) => "string".into(),
            TokenKind::Ident(_) => "identifier".into(),
            TokenKind::Punct(p) => format!("`{}`", p.as_str()),
        }
    }
}

fn strip_quotes(slice: &str) -> &str {
    &slice[1..slice.len().saturating_sub(1)]
}

/// Tokenizes a whole file. Lexing continues after a bad character so the caller
/// gets every error in one pass, not just the first.
pub fn tokenize(src: &str) -> Result<Vec<Token<'_>>, Vec<LexError>> {
    let mut lexer = TokenKind::lexer(src);
    let mut tokens = Vec::new();
    let mut errors = Vec::new();

    while let Some(result) = lexer.next() {
        let span = Span::from(lexer.span());
        match result {
            // Logos backtracks to `/` before `*` when a block comment never
            // closes, so catch that pair here instead of reporting two tokens.
            Ok(TokenKind::Punct(Punct::Slash))
                if src.get(span.end..).is_some_and(|r| r.starts_with('*')) =>
            {
                errors.push(LexError::new(
                    Span::new(span.start, src.len()),
                    LexErrorKind::UnterminatedComment,
                ));
                break;
            }
            Ok(kind) => tokens.push(Token { kind, span }),
            Err(()) => errors.push(LexError::new(span, classify_error(src, span))),
        }
    }

    if errors.is_empty() {
        Ok(tokens)
    } else {
        Err(errors)
    }
}

fn classify_error(src: &str, span: Span) -> LexErrorKind {
    let rest = &src[span.start.min(src.len())..];
    if rest.starts_with('"') || rest.starts_with('\'') {
        LexErrorKind::UnterminatedString
    } else if rest.starts_with("/*") {
        LexErrorKind::UnterminatedComment
    } else {
        LexErrorKind::UnexpectedChar(rest.chars().next().unwrap_or('\0'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<TokenKind<'_>> {
        tokenize(src).unwrap().into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn lexes_a_binding() {
        assert_eq!(
            kinds("let x = 10"),
            vec![
                TokenKind::Keyword(Keyword::Let),
                TokenKind::Ident("x"),
                TokenKind::Punct(Punct::Eq),
                TokenKind::Int("10"),
            ]
        );
    }

    #[test]
    fn longest_match_wins_for_operators() {
        assert_eq!(
            kinds("a?.b ?? c..<d"),
            vec![
                TokenKind::Ident("a"),
                TokenKind::Punct(Punct::QuestionDot),
                TokenKind::Ident("b"),
                TokenKind::Punct(Punct::QuestionQuestion),
                TokenKind::Ident("c"),
                TokenKind::Punct(Punct::DotDotLt),
                TokenKind::Ident("d"),
            ]
        );
    }

    #[test]
    fn ranges_do_not_swallow_the_dot() {
        assert_eq!(
            kinds("1..5"),
            vec![
                TokenKind::Int("1"),
                TokenKind::Punct(Punct::DotDot),
                TokenKind::Int("5")
            ]
        );
        assert_eq!(
            kinds("1..<5"),
            vec![
                TokenKind::Int("1"),
                TokenKind::Punct(Punct::DotDotLt),
                TokenKind::Int("5")
            ]
        );
    }

    #[test]
    fn skips_comments_but_keeps_newlines() {
        let tokens = kinds("# hash\n// slash\nlet x = 1 /* block */\n");
        assert_eq!(
            tokens,
            vec![
                TokenKind::Newline,
                TokenKind::Newline,
                TokenKind::Keyword(Keyword::Let),
                TokenKind::Ident("x"),
                TokenKind::Punct(Punct::Eq),
                TokenKind::Int("1"),
                TokenKind::Newline,
            ]
        );
    }

    #[test]
    fn keeps_the_raw_string_contents() {
        assert_eq!(
            kinds(r#""a\n${b}" '${b}'"#),
            vec![TokenKind::Str(r"a\n${b}"), TokenKind::RawStr("${b}")]
        );
    }

    #[test]
    fn numbers_follow_the_documented_shapes() {
        assert_eq!(
            kinds("0xFF 1_000 3.14 2e5"),
            vec![
                TokenKind::Int("0xFF"),
                TokenKind::Int("1_000"),
                TokenKind::Float("3.14"),
                TokenKind::Float("2e5"),
            ]
        );
    }

    #[test]
    fn reports_unterminated_strings() {
        let errs = tokenize("let x = \"oops").unwrap_err();
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].kind, LexErrorKind::UnterminatedString);
        assert_eq!(errs[0].span.start, 8);
    }

    #[test]
    fn reports_unterminated_block_comments() {
        let errs = tokenize("/* nope").unwrap_err();
        assert_eq!(errs[0].kind, LexErrorKind::UnterminatedComment);
    }

    #[test]
    fn spans_point_at_the_token() {
        let tokens = tokenize("let x").unwrap();
        assert_eq!(tokens[0].span, Span::new(0, 3));
        assert_eq!(tokens[1].span, Span::new(4, 5));
    }

    #[test]
    fn every_newline_style_separates_statements() {
        assert_eq!(
            kinds("a\r\nb\rc\nd"),
            vec![
                TokenKind::Ident("a"),
                TokenKind::Newline,
                TokenKind::Ident("b"),
                TokenKind::Newline,
                TokenKind::Ident("c"),
                TokenKind::Newline,
                TokenKind::Ident("d"),
            ]
        );
    }
}
