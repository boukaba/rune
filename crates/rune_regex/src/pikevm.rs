use crate::nfa::{Edge, Nfa};

pub struct PikeVm;

#[derive(Clone, Debug)]
pub struct Match {
    pub groups: Vec<(usize, usize)>,
}

#[derive(Clone)]
struct Thread {
    pc: usize,
    saves: Vec<Option<usize>>,
}

impl Default for PikeVm {
    fn default() -> Self {
        Self::new()
    }
}

impl PikeVm {
    pub fn new() -> Self {
        PikeVm
    }

    pub fn exec(&self, nfa: &Nfa, text: &str, start: usize) -> Option<Match> {
        let chars: Vec<char> = text.chars().collect();
        if start >= chars.len() {
            return None;
        }

        let num_slots = (nfa.num_captures + 1) * 2;

        for pos in start..chars.len() {
            let mut clist: Vec<Thread> = Vec::new();
            add_thread(&mut clist, nfa, nfa.start, &vec![None; num_slots]);

            let mut longest_match: Option<Match> = None;

            for p in pos..chars.len() {
                if clist.is_empty() {
                    break;
                }

                // Follow Save and Epsilon edges (non-consuming) until fixpoint
                let expanded = follow_nonconsuming(&clist, nfa, p);

                // Check match in expanded threads
                for t in &expanded {
                    if nfa.states[t.pc].is_match {
                        let match_groups = build_groups(&t.saves, pos, p, nfa.num_captures);
                        let should_replace = match &longest_match {
                            None => true,
                            Some(prev) => p > prev.groups[0].1,
                        };
                        if should_replace {
                            longest_match = Some(match_groups);
                        }
                    }
                }

                // Advance threads with current character
                let c = chars[p];
                let mut nlist: Vec<Thread> = Vec::new();
                for t in &expanded {
                    for edge in &nfa.states[t.pc].edges {
                        match edge {
                            Edge::Char(ch, target) => {
                                if *ch == c {
                                    add_thread(&mut nlist, nfa, *target, &t.saves);
                                }
                            }
                            Edge::CharClass { negated, ranges, target } => {
                                let in_class = ranges.iter().any(|(lo, hi)| c >= *lo && c <= *hi);
                                if *negated != in_class {
                                    add_thread(&mut nlist, nfa, *target, &t.saves);
                                }
                            }
                            Edge::Dot(target) => {
                                add_thread(&mut nlist, nfa, *target, &t.saves);
                            }
                            Edge::Epsilon(_) | Edge::Save(_, _) => {}
                        }
                    }
                }
                clist = nlist;
            }

            // Check match at end of string
            let expanded = follow_nonconsuming(&clist, nfa, chars.len());
            for t in &expanded {
                if nfa.states[t.pc].is_match {
                    let end_pos = chars.len();
                    let match_groups = build_groups(&t.saves, pos, end_pos, nfa.num_captures);
                    let should_replace = match &longest_match {
                        None => true,
                        Some(prev) => end_pos > prev.groups[0].1,
                    };
                    if should_replace {
                        longest_match = Some(match_groups);
                    }
                }
            }

            if let Some(m) = longest_match {
                return Some(m);
            }
        }
        None
    }
}

/// Follow all non-consuming edges (Save and Epsilon) until fixpoint.
fn follow_nonconsuming(threads: &[Thread], nfa: &Nfa, pos: usize) -> Vec<Thread> {
    let mut result: Vec<Thread> = Vec::new();
    let mut worklist: Vec<Thread> = threads.to_vec();

    while let Some(t) = worklist.pop() {
        let mut has_nonconsuming = false;
        for edge in &nfa.states[t.pc].edges {
            match edge {
                Edge::Save(slot, target) => {
                    has_nonconsuming = true;
                    let mut saves = t.saves.clone();
                    saves[*slot] = Some(pos);
                    let new_t = Thread { pc: *target, saves };
                    if !in_sets(&new_t, &worklist, &result) {
                        worklist.push(new_t);
                    }
                }
                Edge::Epsilon(target) => {
                    has_nonconsuming = true;
                    let new_t = Thread { pc: *target, saves: t.saves.clone() };
                    if !in_sets(&new_t, &worklist, &result) {
                        worklist.push(new_t);
                    }
                }
                _ => {}
            }
        }
        if !has_nonconsuming && !in_result(&t, &result) {
            result.push(t);
        }
    }
    result
}

fn in_sets(t: &Thread, worklist: &[Thread], result: &[Thread]) -> bool {
    worklist.iter().any(|w| w.pc == t.pc && w.saves == t.saves)
        || result.iter().any(|r| r.pc == t.pc && r.saves == t.saves)
}

fn in_result(t: &Thread, result: &[Thread]) -> bool {
    result.iter().any(|r| r.pc == t.pc && r.saves == t.saves)
}

fn build_groups(saves: &[Option<usize>], match_start: usize, match_end: usize, num_captures: usize) -> Match {
    let mut groups = vec![(0, 0); num_captures + 1];
    groups[0] = (saves[0].unwrap_or(match_start), match_end);
    for i in 0..num_captures {
        let s = i * 2 + 2;
        let e = i * 2 + 3;
        groups[i + 1] = (
            saves[s].unwrap_or(match_end),
            saves[e].unwrap_or(match_end),
        );
    }
    Match { groups }
}

fn add_thread(nlist: &mut Vec<Thread>, nfa: &Nfa, pc: usize, saves: &[Option<usize>]) {
    // Skip if already in nlist (same pc + saves)
    for t in nlist.iter() {
        if t.pc == pc && t.saves == saves {
            return;
        }
    }
    nlist.push(Thread { pc, saves: saves.to_vec() });
    for edge in &nfa.states[pc].edges {
        if let Edge::Epsilon(target) = edge {
            add_thread(nlist, nfa, *target, saves);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_regex;

    #[test]
    fn test_literal_match() {
        let expr = parse_regex("abc").unwrap();
        let nfa = crate::nfa::compile(&expr);
        let vm = PikeVm::new();
        let m = vm.exec(&nfa, "xyzabcdef", 0);
        assert_eq!(m.as_ref().map(|m| m.groups[0]), Some((3, 6)));
    }

    #[test]
    fn test_alt() {
        let expr = parse_regex("a|b").unwrap();
        let nfa = crate::nfa::compile(&expr);
        let vm = PikeVm::new();
        let m = vm.exec(&nfa, "xyz", 0);
        assert!(m.is_none());
        let m = vm.exec(&nfa, "cat", 0);
        assert_eq!(m.as_ref().map(|m| m.groups[0]), Some((1, 2)));
    }

    #[test]
    fn test_star() {
        let expr = parse_regex("a*").unwrap();
        let nfa = crate::nfa::compile(&expr);
        let vm = PikeVm::new();
        let m = vm.exec(&nfa, "bba", 0);
        assert_eq!(m.as_ref().map(|m| m.groups[0]), Some((0, 0)));
        let m = vm.exec(&nfa, "bba", 2);
        assert_eq!(m.as_ref().map(|m| m.groups[0]), Some((2, 3)));
    }

    #[test]
    fn test_dot() {
        let expr = parse_regex("c.t").unwrap();
        let nfa = crate::nfa::compile(&expr);
        let vm = PikeVm::new();
        let m = vm.exec(&nfa, "cat", 0);
        assert_eq!(m.as_ref().map(|m| m.groups[0]), Some((0, 3)));
    }

    #[test]
    fn test_multiple_matches() {
        let expr = parse_regex(r"\.").unwrap();
        let nfa = crate::nfa::compile(&expr);
        let vm = PikeVm::new();
        let m = vm.exec(&nfa, "a.b.c", 0);
        assert_eq!(m.as_ref().map(|m| m.groups[0]), Some((1, 2)));
        let m = vm.exec(&nfa, "a.b.c", 2);
        assert_eq!(m.as_ref().map(|m| m.groups[0]), Some((3, 4)));
        let m = vm.exec(&nfa, "a.b.c", 4);
        assert!(m.is_none());
    }

    #[test]
    fn test_replace_all() {
        let expr = parse_regex(r"\.").unwrap();
        let nfa = crate::nfa::compile(&expr);
        let vm = PikeVm::new();
        let text = "a.b.c";
        let mut results = Vec::new();
        let mut last_end = 0;
        while let Some(m) = vm.exec(&nfa, text, last_end) {
            results.push(m.groups[0]);
            last_end = m.groups[0].1;
        }
        assert_eq!(results, vec![(1, 2), (3, 4)]);
    }

    #[test]
    fn test_char_class() {
        let expr = parse_regex("[0-9]+").unwrap();
        let nfa = crate::nfa::compile(&expr);
        let vm = PikeVm::new();
        let m = vm.exec(&nfa, "abc123def", 0);
        assert_eq!(m.as_ref().map(|m| m.groups[0]), Some((3, 6)));
    }

    #[test]
    fn test_capture_group() {
        let expr = parse_regex(r"(a)(b)").unwrap();
        let nfa = crate::nfa::compile(&expr);
        assert_eq!(nfa.num_captures, 2);
        let vm = PikeVm::new();
        let m = vm.exec(&nfa, "xab", 0).unwrap();
        assert_eq!(m.groups[0], (1, 3));
        assert_eq!(m.groups[1], (1, 2));
        assert_eq!(m.groups[2], (2, 3));
    }

    #[test]
    fn test_nested_capture() {
        let expr = parse_regex(r"(a(b))").unwrap();
        let nfa = crate::nfa::compile(&expr);
        assert_eq!(nfa.num_captures, 2);
        let vm = PikeVm::new();
        let m = vm.exec(&nfa, "xab", 0).unwrap();
        assert_eq!(m.groups[0], (1, 3));
        assert_eq!(m.groups[1], (1, 3));
        assert_eq!(m.groups[2], (2, 3));
    }

    #[test]
    fn test_capture_with_dollar() {
        let expr = parse_regex(r"(hello) (world)").unwrap();
        let nfa = crate::nfa::compile(&expr);
        assert_eq!(nfa.num_captures, 2);
        let vm = PikeVm::new();
        let m = vm.exec(&nfa, "say hello world here", 0).unwrap();
        assert_eq!(m.groups[0], (4, 15));
        assert_eq!(m.groups[1], (4, 9));
        assert_eq!(m.groups[2], (10, 15));
    }
}
