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
    Save(usize, usize),
}

pub struct Nfa {
    pub start: usize,
    pub states: Vec<State>,
    pub num_captures: usize,
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
        RegexExpr::Group(inner, cap_idx) => {
            if cap_idx.is_some() {
                let idx = *group_count;
                *group_count += 1;
                let slot_start = idx * 2 + 2;
                let slot_end = idx * 2 + 3;
                let s_save_start = alloc_state(states);
                let (s_in, s_out) = compile_expr(inner, states, group_count);
                let s_save_end = alloc_state(states);
                states[s_save_end].is_match = true;
                states[s_save_start].edges.push(Edge::Save(slot_start, s_in));
                states[s_out].edges.push(Edge::Save(slot_end, s_save_end));
                states[s_out].is_match = false;
                (s_save_start, s_save_end)
            } else {
                compile_expr(inner, states, group_count)
            }
        }
        RegexExpr::CharClass { negated, ranges } => {
            let s1 = alloc_state(states);
            let s2 = alloc_state(states);
            states[s2].is_match = true;
            states[s1].edges.push(Edge::CharClass { negated: *negated, ranges: ranges.clone(), target: s2 });
            (s1, s2)
        }
        RegexExpr::AnchorStart | RegexExpr::AnchorEnd | RegexExpr::Backref(_) => {
            let s = alloc_state(states);
            (s, s)
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

    Nfa { start: s_start, states, num_captures: group_count }
}
