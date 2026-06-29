#[derive(Clone, Debug, PartialEq)]
pub enum RegexExpr {
    Empty,
    Literal(char),
    Dot,
    Concat(Vec<RegexExpr>),
    Alt(Box<RegexExpr>, Box<RegexExpr>),
    Star(Box<RegexExpr>),
    Plus(Box<RegexExpr>),
    Optional(Box<RegexExpr>),
    Group(Box<RegexExpr>, Option<usize>),
    CharClass { negated: bool, ranges: Vec<(char, char)> },
    AnchorStart,
    AnchorEnd,
    Backref(usize),
}
