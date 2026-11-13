//! Syntax tree produced by the parser.
//!
//! Every node carries a [`Span`] back into the source. Names and string parts
//! own their text, so the tree outlives the input buffer.

use crate::span::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct Script {
    pub stmts: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindKind {
    Let,
    Const,
    Global,
}

/// An optional type annotation such as `number` or `player?`.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeRef {
    pub name: String,
    /// Set when the type ends in `?`.
    pub optional: bool,
    pub span: Span,
}

/// One function parameter. `greeting = "Hello"` gives `default`, `n: number`
/// gives `ty`.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub ty: Option<TypeRef>,
    pub default: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// `let x = 1`, `const name = "hi"`, `global visits = 0`.
    Bind {
        kind: BindKind,
        name: String,
        ty: Option<TypeRef>,
        value: Expr,
        span: Span,
    },
    /// `fun twice(n) { ... }`
    Fn {
        name: String,
        params: Vec<Param>,
        body: Block,
        span: Span,
    },
    /// `x = 1`, `x += 5`, `player.health = 20`, `scores["alice"] = 5`.
    Assign {
        target: Expr,
        op: AssignOp,
        value: Expr,
        span: Span,
    },
    Return {
        value: Option<Expr>,
        span: Span,
    },
    /// `else` is either [`Stmt::Block`] or another [`Stmt::If`], which keeps
    /// `else if` chains flat.
    If {
        cond: Expr,
        then_branch: Block,
        else_branch: Option<Box<Stmt>>,
        span: Span,
    },
    While {
        cond: Expr,
        body: Block,
        span: Span,
    },
    For {
        var: String,
        iter: Expr,
        body: Block,
        span: Span,
    },
    Break {
        span: Span,
    },
    Continue {
        span: Span,
    },
    Block(Block),
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Int {
        value: i64,
        span: Span,
    },
    Float {
        value: f64,
        span: Span,
    },
    Str(StrLit),
    Bool {
        value: bool,
        span: Span,
    },
    Null {
        span: Span,
    },
    Ident {
        name: String,
        span: Span,
    },
    List {
        items: Vec<Expr>,
        span: Span,
    },
    Map {
        entries: Vec<MapEntry>,
        span: Span,
    },
    /// `foo(...)`, `player.send(...)`, `ScrippyAI.ask(...)`.
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        span: Span,
    },
    /// `player.name`, `event.item`, `a?.b`.
    Member {
        object: Box<Expr>,
        name: String,
        /// True for `?.` — the access short-circuits to null when the object is null.
        safe: bool,
        span: Span,
    },
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
        span: Span,
    },
    /// `new ItemStack(material("DIAMOND_SWORD"), 1)`.
    New {
        name: String,
        args: Vec<Expr>,
        span: Span,
    },
    /// `fun(a, b) { return a + b }`
    Func {
        params: Vec<Param>,
        body: Block,
        span: Span,
    },
    Unary {
        op: UnOp,
        expr: Box<Expr>,
        span: Span,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        span: Span,
    },
    Ternary {
        cond: Box<Expr>,
        then_expr: Box<Expr>,
        else_expr: Box<Expr>,
        span: Span,
    },
}

/// A string literal, already split into plain text and `${...}` expressions.
#[derive(Debug, Clone, PartialEq)]
pub struct StrLit {
    pub parts: Vec<StrPart>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StrPart {
    Text(String),
    Expr(Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MapEntry {
    pub key: String,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Coalesce,
    /// `1..5`, inclusive on both ends.
    Range,
    /// `0..<10`.
    RangeExcl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    Assign,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

impl Stmt {
    pub fn span(&self) -> Span {
        match self {
            Stmt::Bind { span, .. }
            | Stmt::Fn { span, .. }
            | Stmt::Assign { span, .. }
            | Stmt::Return { span, .. }
            | Stmt::If { span, .. }
            | Stmt::While { span, .. }
            | Stmt::For { span, .. }
            | Stmt::Break { span }
            | Stmt::Continue { span } => *span,
            Stmt::Block(block) => block.span,
            Stmt::Expr(expr) => expr.span(),
        }
    }
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Int { span, .. }
            | Expr::Float { span, .. }
            | Expr::Bool { span, .. }
            | Expr::Null { span }
            | Expr::Ident { span, .. }
            | Expr::List { span, .. }
            | Expr::Map { span, .. }
            | Expr::Call { span, .. }
            | Expr::Member { span, .. }
            | Expr::Index { span, .. }
            | Expr::New { span, .. }
            | Expr::Func { span, .. }
            | Expr::Unary { span, .. }
            | Expr::Binary { span, .. }
            | Expr::Ternary { span, .. } => *span,
            Expr::Str(lit) => lit.span,
        }
    }
}
