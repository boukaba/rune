use std::fmt;

/// Source location span.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArrayElement {
    pub expr: Expr,
    pub is_spread: bool,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Number(f64, Span),
    String(Box<str>, Span),
    Boolean(bool, Span),
    Null(Span),
    Undefined(Span),
    Identifier(Box<str>, Span),
    Array(Vec<ArrayElement>, Span),
    Object(Vec<Property>, Span),
    Unary(UnaryOp, Box<Expr>, Span),
    Binary(BinaryOp, Box<Expr>, Box<Expr>, Span),
    Conditional(Box<Expr>, Box<Expr>, Box<Expr>, Span),
    Call(Box<Expr>, Vec<ArrayElement>, Span),
    New(Box<Expr>, Vec<ArrayElement>, Span),
    Member(Box<Expr>, Box<Expr>, bool, Span), // computed = true for a[b]
    Assign(Box<Expr>, Box<Expr>, Span),
    CompoundAssign(BinaryOp, Box<Expr>, Box<Expr>, Span),
    Function(Box<FnNode>, Span),
    Template {
        parts: Vec<String>,
        exprs: Vec<Expr>,
        span: Span,
    },
    This(Span),
    Update(UpdateOp, Box<Expr>, bool, Span), // op, argument, is_prefix, span
    Yield(Option<Box<Expr>>, Span),
    Await(Box<Expr>, Span),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Property {
    pub key: PropKey,
    pub value: Expr,
    pub is_spread: bool,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PropKey {
    String(Box<str>),
    Number(f64),
    Identifier(Box<str>),
    Computed(Box<Expr>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum UnaryOp {
    Plus,
    Minus,
    Not,
    BitNot,
    Typeof,
    Void,
    Delete,
}

/// Increment/decrement update expression.
#[derive(Clone, Debug, PartialEq)]
pub enum UpdateOp {
    PlusPlus,
    MinusMinus,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum BinaryOp {
    // Assignment
    Assign,
    // Comma (lowest precedence)
    Comma,
    // Logical
    LogicalOr,
    LogicalAnd,
    // Bitwise
    BitOr,
    BitXor,
    BitAnd,
    // Equality
    Eq,
    Ne,
    StrictEq,
    StrictNe,
    // Relational
    Lt,
    Gt,
    Le,
    Ge,
    Instanceof,
    In,
    // Shift
    Shl,
    Shr,
    ShrU,
    // Additive
    Add,
    Sub,
    // Multiplicative
    Mul,
    Div,
    Mod,
    Exp,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FnNode {
    pub name: Option<Box<str>>,
    pub params: Vec<Pattern>,
    pub rest_param: Option<Box<str>>,
    pub body: Stmt,
    pub is_generator: bool,
    pub is_async: bool,
    pub is_arrow: bool,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    Expr(Expr, Span),
    Block(Vec<Stmt>, Span),
    If(Box<Expr>, Box<Stmt>, Option<Box<Stmt>>, Span),
    While(Box<Expr>, Box<Stmt>, Span),
    DoWhile(Box<Expr>, Box<Stmt>, Span),
    For(
        Option<Box<Stmt>>,
        Option<Box<Expr>>,
        Option<Box<Expr>>,
        Box<Stmt>,
        Span,
    ),
    ForIn(Box<Expr>, Box<Expr>, Box<Stmt>, Span),
    Var(VarKind, Vec<Decl>, Span),
    Return(Option<Box<Expr>>, Span),
    Throw(Box<Expr>, Span),
    Break(Option<Box<str>>, Span),
    Continue(Option<Box<str>>, Span),
    Try(Box<[Stmt]>, Option<CatchClause>, Option<Box<[Stmt]>>, Span),
    Switch(Box<Expr>, Vec<SwitchCase>, Option<Box<[Stmt]>>, Span),
    Function(Box<FnNode>, Span),
    Empty(Span),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SwitchCase {
    pub test: Expr,
    pub body: Vec<Stmt>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CatchClause {
    pub param: Box<str>,
    pub body: Box<[Stmt]>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum VarKind {
    Var,
    Let,
    Const,
}

/// A binding pattern for destructuring.
#[derive(Clone, Debug, PartialEq)]
pub enum Pattern {
    Identifier(Box<str>, Span, Option<Box<Expr>>),
    Object(Vec<ObjectPatternProp>, Option<Box<Pattern>>, Span),
    Array(Vec<Option<Pattern>>, Span),
    Default(Box<Pattern>, Box<Expr>),
    Rest(Box<Pattern>, Span),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObjectPatternProp {
    pub key: PropKey,
    pub pattern: Pattern,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Decl {
    pub name: Box<str>,
    pub pattern: Option<Pattern>,
    pub init: Option<Box<Expr>>,
    pub span: Span,
}

/// A parsed program (top-level statements).
#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    pub body: Vec<Stmt>,
    pub span: Span,
}

impl fmt::Display for BinaryOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
