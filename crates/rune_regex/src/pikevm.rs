use crate::nfa::{Edge, Nfa};

pub struct PikeVm;

impl Default for PikeVm {
    fn default() -> Self {
        Self::new()
    }
}

impl PikeVm {
    pub fn new() -> Self {
        PikeVm
    }

    /// Find the leftmost-longest match in `text` starting at or after `start`.
    /// Returns `(start, end)` byte offsets of the match, or `None`.
    pub fn exec(&self, nfa: &Nfa, text: &str, start: usize) -> Option<(usize, usize)> {
        let chars: Vec<char> = text.chars().collect();
        if start >= chars.len() {
            return None;
        }

        // Try each position as the start of a match
        for pos in start..chars.len() {
            let mut clist: Vec<usize> = Vec::new();
            add_state(&mut clist, nfa, nfa.start);

            let mut match_end: Option<usize> = None;

            // Simulate forward from pos
            for p in pos..chars.len() {
                if clist.is_empty() {
                    break;
                }
                // Check if any thread reached a match state (before consuming char at p)
                if clist.iter().any(|&pc| nfa.states[pc].is_match) {
                    match_end = Some(match_end.map_or(p, |prev| prev.max(p)));
                }
                // Advance each thread with the current character
                let c = chars[p];
                let mut nlist = Vec::new();
                for &pc in &clist {
                    for edge in &nfa.states[pc].edges {
                        match edge {
                            Edge::Char(ch, target) => {
                                if *ch == c {
                                    add_state(&mut nlist, nfa, *target);
                                }
                            }
                            Edge::CharClass { negated, ranges, target } => {
                                let in_class = ranges.iter().any(|(lo, hi)| c >= *lo && c <= *hi);
                                if *negated != in_class {
                                    add_state(&mut nlist, nfa, *target);
                                }
                            }
                            Edge::Dot(target) => {
                                add_state(&mut nlist, nfa, *target);
                            }
                            Edge::Epsilon(_) => {} // epsilon already followed in add_state
                        }
                    }
                }
                clist = nlist;
            }
            // Check match at end of string
            if clist.iter().any(|&pc| nfa.states[pc].is_match) {
                match_end = Some(chars.len());
            }

            if let Some(end) = match_end {
                return Some((pos, end));
            }
        }
        None
    }
}

fn add_state(nlist: &mut Vec<usize>, nfa: &Nfa, pc: usize) {
    if nlist.contains(&pc) {
        return;
    }
    nlist.push(pc);
    for edge in &nfa.states[pc].edges {
        if let Edge::Epsilon(target) = edge {
            add_state(nlist, nfa, *target);
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
        assert_eq!(m, Some((3, 6)));
    }

    #[test]
    fn test_alt() {
        let expr = parse_regex("a|b").unwrap();
        let nfa = crate::nfa::compile(&expr);
        let vm = PikeVm::new();
        let m = vm.exec(&nfa, "xyz", 0);
        assert_eq!(m, None);
        let m = vm.exec(&nfa, "cat", 0);
        assert_eq!(m, Some((1, 2)));
    }

    #[test]
    fn test_star() {
        let expr = parse_regex("a*").unwrap();
        let nfa = crate::nfa::compile(&expr);
        let vm = PikeVm::new();
        let m = vm.exec(&nfa, "bba", 0);
        assert_eq!(m, Some((0, 0))); // matches zero a's at position 0
        let m = vm.exec(&nfa, "bba", 2);
        assert_eq!(m, Some((2, 3))); // matches one a at position 2
    }

    #[test]
    fn test_dot() {
        let expr = parse_regex("c.t").unwrap();
        let nfa = crate::nfa::compile(&expr);
        let vm = PikeVm::new();
        let m = vm.exec(&nfa, "cat", 0);
        assert_eq!(m, Some((0, 3)));
    }

    #[test]
    fn test_multiple_matches() {
        let expr = parse_regex(r"\.").unwrap();
        let nfa = crate::nfa::compile(&expr);
        let vm = PikeVm::new();
        let m = vm.exec(&nfa, "a.b.c", 0);
        assert_eq!(m, Some((1, 2)));
        let m = vm.exec(&nfa, "a.b.c", 2);
        assert_eq!(m, Some((3, 4)));
        let m = vm.exec(&nfa, "a.b.c", 4);
        assert_eq!(m, None);
    }

    #[test]
    fn test_replace_all() {
        let expr = parse_regex(r"\.").unwrap();
        let nfa = crate::nfa::compile(&expr);
        let vm = PikeVm::new();
        let text = "a.b.c";
        let mut results = Vec::new();
        let mut last_end = 0;
        while let Some((s, e)) = vm.exec(&nfa, text, last_end) {
            results.push((s, e));
            last_end = e;
        }
        assert_eq!(results, vec![(1, 2), (3, 4)]);
    }

    #[test]
    fn test_char_class() {
        let expr = parse_regex("[0-9]+").unwrap();
        let nfa = crate::nfa::compile(&expr);
        let vm = PikeVm::new();
        let m = vm.exec(&nfa, "abc123def", 0);
        assert_eq!(m, Some((3, 6)));
    }
}
