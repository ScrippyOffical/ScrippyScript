//! Front end for ScrippyScript (`.scp`): lexer, parser and syntax tree.
//!
//! No I/O, no threads, no unsafe. Builds for `wasm32-unknown-unknown` and runs
//! unchanged inside a browser or the Minecraft mod.
//!
//! ```
//! let script = scrippy_script_core::parse("let x = 1 + 2").unwrap();
//! assert_eq!(script.stmts.len(), 1);
//! ```
//!
//! For better diagnostics, keep the source around and render errors with
//! [`Error::render`]:
//!
//! ```
//! let src = "let x = ";
//! let errors = scrippy_script_core::parse(src).unwrap_err();
//! let text = errors[0].render(src);
//! assert!(text.contains("unexpected end of input"));
//! ```

#![forbid(unsafe_code)]

use std::fmt;

pub mod ast;
pub mod error;
pub mod lexer;
pub mod parser;
pub mod span;

pub use ast::{
    AssignOp, BinOp, BindKind, Block, Expr, MapEntry, Param, Script, Stmt, StrLit, StrPart,
    TypeRef, UnOp,
};
pub use error::{LexError, LexErrorKind, ParseError, ParseErrorKind};
pub use lexer::{tokenize, Keyword, Punct, Token, TokenKind};
pub use parser::parse as parse_tokens;
pub use span::Span;

/// A lexing or parsing failure. Parsing stops at the first syntax error, so at
/// most one [`Error::Parse`] is returned. Lexical errors are collected for the
/// whole file, except that an unterminated block comment ends the pass because
/// everything after it would be read as comment text.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    Lex(LexError),
    Parse(ParseError),
}

impl Error {
    pub fn span(&self) -> Span {
        match self {
            Error::Lex(err) => err.span,
            Error::Parse(err) => err.span,
        }
    }

    /// Error text with the offending source line and a caret.
    pub fn render(&self, src: &str) -> String {
        match self {
            Error::Lex(err) => err.render(src),
            Error::Parse(err) => err.render(src),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Lex(err) => err.fmt(f),
            Error::Parse(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for Error {}

/// Lexes and parses a complete `.scp` file.
pub fn parse(src: &str) -> Result<Script, Vec<Error>> {
    let tokens = match tokenize(src) {
        Ok(tokens) => tokens,
        Err(errors) => return Err(errors.into_iter().map(Error::Lex).collect()),
    };
    parser::parse(tokens).map_err(|err| vec![Error::Parse(err)])
}
