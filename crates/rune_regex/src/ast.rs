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
    CharClass {
        negated: bool,
        ranges: Vec<(char, char)>,
    },
    AnchorStart,
    AnchorEnd,
    WordBoundary {
        negated: bool,
    },
    Backref(usize),
    /// Zero-width lookahead assertion: the inner expression must match
    /// starting at the current position (negated = negative lookahead).
    /// Captures inside a positive lookahead participate in the match.
    Lookahead {
        expr: Box<RegexExpr>,
        negated: bool,
    },
}
