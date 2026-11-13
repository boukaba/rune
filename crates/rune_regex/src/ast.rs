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
    Group(Box<RegexExpr>, Option<usize>, Option<String>),
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
    /// Named backreference resolved to `Backref` after parsing.
    NamedBackref(String),
    /// Backreference that may refer to any of several duplicate-named captures.
    BackrefMulti(Vec<usize>),
    /// Zero-width reset of capture slots (start of a repeated copy).
    ClearCaptures(Vec<usize>),
    /// Lazy quantifier marker (shortest-match preference).
    Lazy(Box<RegexExpr>),
    /// Zero-width lookahead assertion: the inner expression must match
    /// starting at the current position (negated = negative lookahead).
    /// Captures inside a positive lookahead participate in the match.
    Lookahead {
        expr: Box<RegexExpr>,
        negated: bool,
    },
}

/// Collect named capture groups (name, capture index) in source order.
pub fn named_captures(expr: &RegexExpr) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    walk_named(expr, &mut out);
    out
}

fn walk_named(expr: &RegexExpr, out: &mut Vec<(String, usize)>) {
    match expr {
        RegexExpr::Group(inner, cap, name) => {
            if let (Some(idx), Some(n)) = (cap, name) {
                out.push((n.clone(), *idx));
            }
            walk_named(inner, out);
        }
        RegexExpr::Concat(parts) => {
            for p in parts {
                walk_named(p, out);
            }
        }
        RegexExpr::Alt(a, b) => {
            walk_named(a, out);
            walk_named(b, out);
        }
        RegexExpr::Star(e)
        | RegexExpr::Plus(e)
        | RegexExpr::Optional(e)
        | RegexExpr::Lazy(e)
        | RegexExpr::Lookahead { expr: e, .. } => walk_named(e, out),
        _ => {}
    }
}

/// All capture indices contained in `expr`.
pub fn capture_indices(expr: &RegexExpr) -> Vec<usize> {
    let mut out = Vec::new();
    collect_caps(expr, &mut out);
    out
}

fn collect_caps(expr: &RegexExpr, out: &mut Vec<usize>) {
    match expr {
        RegexExpr::Group(inner, cap, _) => {
            if let Some(idx) = cap {
                if !out.contains(idx) {
                    out.push(*idx);
                }
            }
            collect_caps(inner, out);
        }
        RegexExpr::Concat(parts) => {
            for p in parts {
                collect_caps(p, out);
            }
        }
        RegexExpr::Alt(a, b) => {
            collect_caps(a, out);
            collect_caps(b, out);
        }
        RegexExpr::Star(e)
        | RegexExpr::Plus(e)
        | RegexExpr::Optional(e)
        | RegexExpr::Lazy(e)
        | RegexExpr::Lookahead { expr: e, .. } => collect_caps(e, out),
        _ => {}
    }
}
