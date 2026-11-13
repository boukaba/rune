use crate::ast::RegexExpr;

#[derive(Clone, Debug)]
pub struct State {
    pub is_match: bool,
    pub edges: Vec<Edge>,
}

#[derive(Clone, Debug)]
pub enum Edge {
    Char(char, usize),
    CharClass {
        negated: bool,
        ranges: Vec<(char, char)>,
        target: usize,
    },
    Epsilon(usize),
    Dot(usize),
    Save(usize, usize),
    /// Zero-width lookahead assertion. At the current position, run the
    /// sub-automaton rooted at `sub_start`; it matches when a thread reaches
    /// `sub_match`. On success (or failure for a negated lookahead) continue
    /// at `target` with the same position.
    Lookahead {
        sub_start: usize,
        sub_match: usize,
        negated: bool,
        target: usize,
    },
    /// Zero-width anchor assertions (preserve Thompson NFA→PikeVM arch; checked at current pos).
    AnchorStart(usize),
    AnchorEnd(usize),
    /// Word boundary assertion: `\b` (negated=false) or `\B` (negated=true).
    WordBoundary {
        negated: bool,
        target: usize,
    },
    /// Backreference to capture group `index` (1-indexed).
    Backref {
        index: usize,
        target: usize,
    },
    /// Backreference matching any of several duplicate-named captures.
    BackrefMulti {
        indices: Vec<usize>,
        target: usize,
    },
    /// Reset capture slots to None, then continue (zero-width).
    Clear {
        slots: Vec<usize>,
        target: usize,
    },
}

pub struct Nfa {
    pub start: usize,
    pub states: Vec<State>,
    pub num_captures: usize,
    pub anchored_start: bool,
    pub has_lazy: bool,
}

fn contains_lazy(expr: &RegexExpr) -> bool {
    match expr {
        RegexExpr::Lazy(_) => true,
        RegexExpr::Group(inner, _, _) => contains_lazy(inner),
        RegexExpr::Concat(parts) => parts.iter().any(contains_lazy),
        RegexExpr::Alt(a, b) => contains_lazy(a) || contains_lazy(b),
        RegexExpr::Star(e) | RegexExpr::Plus(e) | RegexExpr::Optional(e) => contains_lazy(e),
        RegexExpr::Lookahead { expr, .. } => contains_lazy(expr),
        _ => false,
    }
}

fn starts_anchored(expr: &RegexExpr) -> bool {
    match expr {
        RegexExpr::AnchorStart => true,
        RegexExpr::Group(inner, _, _) => starts_anchored(inner),
        RegexExpr::Concat(parts) => parts.first().is_some_and(starts_anchored),
        RegexExpr::Alt(a, b) => starts_anchored(a) && starts_anchored(b),
        _ => false,
    }
}

fn alloc_state(states: &mut Vec<State>) -> usize {
    let id = states.len();
    states.push(State {
        is_match: false,
        edges: Vec::new(),
    });
    id
}

fn compile_expr(
    expr: &RegexExpr,
    states: &mut Vec<State>,
    group_count: &mut usize,
) -> (usize, usize) {
    match expr {
        RegexExpr::Empty => {
            let s = alloc_state(states);
            (s, s)
        }
        RegexExpr::Literal(c) => {
            let s1 = alloc_state(states);
            let s2 = alloc_state(states);
            states[s2].is_match = true;
            states[s1].edges.push(Edge::Char(*c, s2));
            (s1, s2)
        }
        RegexExpr::Dot => {
            let s1 = alloc_state(states);
            let s2 = alloc_state(states);
            states[s2].is_match = true;
            states[s1].edges.push(Edge::Dot(s2));
            (s1, s2)
        }
        RegexExpr::Concat(nodes) => {
            let mut last_out = None;
            let mut first_in = None;
            for node in nodes {
                let (s1, s2) = compile_expr(node, states, group_count);
                if let Some(prev_out) = last_out {
                    let idx: usize = prev_out;
                    states[idx].edges.push(Edge::Epsilon(s1));
                    states[idx].is_match = false;
                } else {
                    first_in = Some(s1);
                }
                last_out = Some(s2);
            }
            (first_in.unwrap(), last_out.unwrap())
        }
        RegexExpr::Alt(a, b) => {
            let s_start = alloc_state(states);
            let (s1a, s1b) = compile_expr(a, states, group_count);
            let (s2a, s2b) = compile_expr(b, states, group_count);
            let s_end = alloc_state(states);
            states[s_end].is_match = true;
            states[s_start].edges.push(Edge::Epsilon(s1a));
            states[s_start].edges.push(Edge::Epsilon(s2a));
            states[s1b].edges.push(Edge::Epsilon(s_end));
            states[s1b].is_match = false;
            states[s2b].edges.push(Edge::Epsilon(s_end));
            states[s2b].is_match = false;
            (s_start, s_end)
        }
        RegexExpr::Lazy(inner) => compile_expr(inner, states, group_count),
        RegexExpr::Star(inner) => {
            let s_start = alloc_state(states);
            let (s_in, s_out) = compile_expr(inner, states, group_count);
            let s_end = alloc_state(states);
            states[s_end].is_match = true;
            states[s_start].edges.push(Edge::Epsilon(s_in));
            states[s_start].edges.push(Edge::Epsilon(s_end));
            states[s_out].edges.push(Edge::Epsilon(s_in));
            states[s_out].is_match = false;
            states[s_out].edges.push(Edge::Epsilon(s_end));
            (s_start, s_end)
        }
        RegexExpr::Plus(inner) => {
            let (s_in, s_out) = compile_expr(inner, states, group_count);
            let s_start = s_in;
            let s_end = alloc_state(states);
            states[s_end].is_match = true;
            states[s_out].edges.push(Edge::Epsilon(s_in));
            states[s_out].is_match = false;
            states[s_out].edges.push(Edge::Epsilon(s_end));
            (s_start, s_end)
        }
        RegexExpr::Optional(inner) => {
            let s_start = alloc_state(states);
            let (s_in, s_out) = compile_expr(inner, states, group_count);
            let s_end = alloc_state(states);
            states[s_end].is_match = true;
            states[s_start].edges.push(Edge::Epsilon(s_end));
            states[s_start].edges.push(Edge::Epsilon(s_in));
            states[s_out].edges.push(Edge::Epsilon(s_end));
            states[s_out].is_match = false;
            (s_start, s_end)
        }
        RegexExpr::Group(inner, cap_idx, _name) => {
            if let Some(idx) = *cap_idx {
                *group_count = (*group_count).max(idx + 1);
                let slot_start = idx * 2 + 2;
                let slot_end = idx * 2 + 3;
                let s_save_start = alloc_state(states);
                let (s_in, s_out) = compile_expr(inner, states, group_count);
                let s_save_end = alloc_state(states);
                states[s_save_end].is_match = true;
                states[s_save_start]
                    .edges
                    .push(Edge::Save(slot_start, s_in));
                states[s_out].edges.push(Edge::Save(slot_end, s_save_end));
                states[s_out].is_match = false;
                (s_save_start, s_save_end)
            } else {
                compile_expr(inner, states, group_count)
            }
        }
        RegexExpr::Lookahead { expr, negated } => {
            let (s_in, s_out) = compile_expr(expr, states, group_count);
            let s_start = alloc_state(states);
            let s_end = alloc_state(states);
            states[s_end].is_match = true;
            states[s_start].edges.push(Edge::Lookahead {
                sub_start: s_in,
                sub_match: s_out,
                negated: *negated,
                target: s_end,
            });
            (s_start, s_end)
        }
        RegexExpr::CharClass { negated, ranges } => {
            let s1 = alloc_state(states);
            let s2 = alloc_state(states);
            states[s2].is_match = true;
            states[s1].edges.push(Edge::CharClass {
                negated: *negated,
                ranges: ranges.clone(),
                target: s2,
            });
            (s1, s2)
        }
        RegexExpr::AnchorStart => {
            let s1 = alloc_state(states);
            let s2 = alloc_state(states);
            states[s2].is_match = true;
            states[s1].edges.push(Edge::AnchorStart(s2));
            (s1, s2)
        }
        RegexExpr::AnchorEnd => {
            let s1 = alloc_state(states);
            let s2 = alloc_state(states);
            states[s2].is_match = true;
            states[s1].edges.push(Edge::AnchorEnd(s2));
            (s1, s2)
        }
        RegexExpr::WordBoundary { negated } => {
            let s1 = alloc_state(states);
            let s2 = alloc_state(states);
            states[s2].is_match = true;
            states[s1].edges.push(Edge::WordBoundary {
                negated: *negated,
                target: s2,
            });
            (s1, s2)
        }
        RegexExpr::NamedBackref(_) => {
            // Unresolved named backrefs are rejected by parse_regex.
            unreachable!("named backreference survived parsing")
        }
        RegexExpr::Backref(idx) => {
            let s1 = alloc_state(states);
            let s2 = alloc_state(states);
            states[s2].is_match = true;
            states[s1].edges.push(Edge::Backref {
                index: *idx,
                target: s2,
            });
            (s1, s2)
        }
        RegexExpr::ClearCaptures(indices) => {
            let s1 = alloc_state(states);
            let s2 = alloc_state(states);
            states[s2].is_match = true;
            let mut slots: Vec<usize> = Vec::new();
            for i in indices {
                slots.push(i * 2 + 2);
                slots.push(i * 2 + 3);
            }
            states[s1].edges.push(Edge::Clear { slots, target: s2 });
            (s1, s2)
        }
        RegexExpr::BackrefMulti(indices) => {
            let s1 = alloc_state(states);
            let s2 = alloc_state(states);
            states[s2].is_match = true;
            states[s1].edges.push(Edge::BackrefMulti {
                indices: indices.clone(),
                target: s2,
            });
            (s1, s2)
        }
    }
}

pub fn compile(expr: &RegexExpr) -> Nfa {
    let mut states = Vec::new();
    let mut group_count = 0;

    let s_start = alloc_state(&mut states);
    let (s_inner, s_inner_end) = compile_expr(expr, &mut states, &mut group_count);
    let s_end = alloc_state(&mut states);
    states[s_end].is_match = true;

    states[s_start].edges.push(Edge::Save(0, s_inner));
    states[s_inner_end].edges.push(Edge::Save(1, s_end));
    states[s_inner_end].is_match = false;

    Nfa {
        start: s_start,
        states,
        num_captures: group_count,
        anchored_start: starts_anchored(expr),
        has_lazy: contains_lazy(expr),
    }
}
