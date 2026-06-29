use crate::ast::RegexExpr;

#[derive(Clone, Debug)]
pub struct State {
    pub is_match: bool,
    pub edges: Vec<Edge>,
}

#[derive(Clone, Debug)]
pub enum Edge {
    Char(char, usize),
    CharClass { negated: bool, ranges: Vec<(char, char)>, target: usize },
    Epsilon(usize),
    Dot(usize),
}

pub struct Nfa {
    pub start: usize,
    pub states: Vec<State>,
}

fn alloc_state(states: &mut Vec<State>) -> usize {
    let id = states.len();
    states.push(State { is_match: false, edges: Vec::new() });
    id
}

fn compile_expr(expr: &RegexExpr, states: &mut Vec<State>, group_count: &mut usize) -> (usize, usize) {
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
        RegexExpr::Star(inner) => {
            let s_start = alloc_state(states);
            let (s_in, s_out) = compile_expr(inner, states, group_count);
            let s_end = alloc_state(states);
            states[s_end].is_match = true;
            // s_start → epsilon → s_in (one or more times)
            states[s_start].edges.push(Edge::Epsilon(s_in));
            // s_start → epsilon → s_end (zero times)
            states[s_start].edges.push(Edge::Epsilon(s_end));
            // s_out → epsilon → s_in (loop back)
            states[s_out].edges.push(Edge::Epsilon(s_in));
            states[s_out].is_match = false;
            // s_out → epsilon → s_end (exit)
            states[s_out].edges.push(Edge::Epsilon(s_end));
            (s_start, s_end)
        }
        RegexExpr::Plus(inner) => {
            let (s_in, s_out) = compile_expr(inner, states, group_count);
            let s_start = s_in;
            let s_end = alloc_state(states);
            states[s_end].is_match = true;
            // s_out → epsilon → s_in (loop back for more)
            states[s_out].edges.push(Edge::Epsilon(s_in));
            states[s_out].is_match = false;
            // s_out → epsilon → s_end (exit after at least one)
            states[s_out].edges.push(Edge::Epsilon(s_end));
            (s_start, s_end)
        }
        RegexExpr::Optional(inner) => {
            let s_start = alloc_state(states);
            let (s_in, s_out) = compile_expr(inner, states, group_count);
            let s_end = alloc_state(states);
            states[s_end].is_match = true;
            // Skip the inner expression
            states[s_start].edges.push(Edge::Epsilon(s_end));
            // Or take it
            states[s_start].edges.push(Edge::Epsilon(s_in));
            states[s_out].edges.push(Edge::Epsilon(s_end));
            states[s_out].is_match = false;
            (s_start, s_end)
        }
        RegexExpr::Group(inner, cap_idx) => {
            let _ = cap_idx; // capture group indices tracked by caller
            let (s1, s2) = compile_expr(inner, states, group_count);
            (s1, s2)
        }
        RegexExpr::CharClass { negated, ranges } => {
            let s1 = alloc_state(states);
            let s2 = alloc_state(states);
            states[s2].is_match = true;
            states[s1].edges.push(Edge::CharClass { negated: *negated, ranges: ranges.clone(), target: s2 });
            (s1, s2)
        }
        RegexExpr::AnchorStart | RegexExpr::AnchorEnd | RegexExpr::Backref(_) => {
            // Simplified: treat anchors/backrefs as epsilon (match anything)
            let s = alloc_state(states);
            (s, s)
        }
    }
}

pub fn compile(expr: &RegexExpr) -> Nfa {
    let mut states = Vec::new();
    let mut group_count = 0;
    let (start, end) = compile_expr(expr, &mut states, &mut group_count);
    states[end].is_match = true;
    Nfa { start, states }
}
