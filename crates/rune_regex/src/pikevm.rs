use std::collections::HashMap;

use crate::nfa::{Edge, Nfa};

pub struct PikeVm;

#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    pub groups: Vec<(usize, usize)>,
}

#[derive(Clone)]
struct Thread {
    pc: usize,
    saves: Vec<Option<usize>>,
}

#[inline]
fn is_word_char(c: char) -> bool {
    matches!(c, 'A'..='Z' | 'a'..='z' | '0'..='9' | '_')
}

impl Default for PikeVm {
    fn default() -> Self {
        Self::new()
    }
}

/// Collect the consuming edges reachable from the start without consuming.
/// `None` means no safe filter (empty match possible or data-dependent edge).
fn first_edges(nfa: &Nfa) -> Option<Vec<Edge>> {
    let mut out = Vec::new();
    let mut seen = vec![false; nfa.states.len()];
    let mut stack = vec![nfa.start];
    while let Some(pc) = stack.pop() {
        if seen[pc] {
            continue;
        }
        seen[pc] = true;
        let state = &nfa.states[pc];
        if state.is_match {
            return None;
        }
        for edge in &state.edges {
            match edge {
                Edge::Char(..) | Edge::CharClass { .. } | Edge::Dot(..) => out.push(edge.clone()),
                Edge::Save(_, t) | Edge::Epsilon(t) | Edge::Clear { target: t, .. } => {
                    stack.push(*t)
                }
                Edge::WordBoundary { target, .. } => stack.push(*target),
                Edge::AnchorStart(t) | Edge::AnchorEnd(t) => stack.push(*t),
                Edge::Lookahead { target, .. } => stack.push(*target),
                Edge::Backref { .. } | Edge::BackrefMulti { .. } => return None,
            }
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

fn first_edge_matches(edge: &Edge, c: char, flags: u32) -> bool {
    match edge {
        Edge::Char(ch, _) => {
            if flags & 2 != 0 {
                ch.eq_ignore_ascii_case(&c)
            } else {
                *ch == c
            }
        }
        Edge::CharClass {
            negated, ranges, ..
        } => {
            let mut inside = ranges.iter().any(|(lo, hi)| c >= *lo && c <= *hi);
            if flags & 2 != 0 && !inside {
                let lower = c.to_ascii_lowercase();
                let upper = c.to_ascii_uppercase();
                inside = ranges
                    .iter()
                    .any(|(lo, hi)| lower >= *lo && lower <= *hi || upper >= *lo && upper <= *hi);
            }
            *negated != inside
        }
        Edge::Dot(_) => flags & 8 != 0 || c != '\n',
        _ => true,
    }
}

impl PikeVm {
    pub fn new() -> Self {
        PikeVm
    }

    pub fn exec(&self, nfa: &Nfa, text: &str, start: usize) -> Option<Match> {
        self.exec_with_flags(nfa, text, start, 0)
    }

    pub fn exec_with_flags(
        &self,
        nfa: &Nfa,
        text: &str,
        start: usize,
        flags: u32,
    ) -> Option<Match> {
        let chars: Vec<char> = text.chars().collect();
        let mut char_bytes = Vec::with_capacity(chars.len() + 1);
        let mut byte_pos = 0usize;
        for c in &chars {
            char_bytes.push(byte_pos);
            byte_pos += c.len_utf8();
        }
        char_bytes.push(byte_pos);
        // Callers pass byte offsets (JS string indices in Rune's UTF-8 model).
        let start = if start == 0 {
            0
        } else {
            let b = start.min(text.len());
            let mut boundary = b;
            while boundary > 0 && !text.is_char_boundary(boundary) {
                boundary -= 1;
            }
            text[..boundary].chars().count()
        };
        if start > chars.len() {
            return None;
        }

        let num_slots = (nfa.num_captures + 1) * 2;

        // Leftmost start loop — try each position as match start (ECMAScript leftmost).
        // A leading ^ can only match at position 0.
        let anchored = nfa.anchored_start && flags & 4 == 0;
        let end_pos = if anchored { 0 } else { chars.len() };
        if anchored && start > 0 {
            return None;
        }
        let first = first_edges(nfa);
        for pos in start..=end_pos {
            if let Some(edges) = &first {
                if pos < chars.len()
                    && !edges
                        .iter()
                        .any(|e| first_edge_matches(e, chars[pos], flags))
                {
                    continue;
                }
            }
            let remaining = chars.len() - pos;
            // Sparse queues: only offsets that actually received a thread are
            // allocated. Dense queues made a failing scan O(n^2) memory.
            let mut queues: HashMap<usize, Vec<Thread>> = HashMap::new();
            queues.entry(0).or_default();
            add_thread(
                queues.get_mut(&0).unwrap(),
                nfa,
                nfa.start,
                &vec![None; num_slots],
            );
            let mut longest: Option<Match> = None;

            let mut offset = 0usize;
            while offset <= remaining {
                let cur_pos = pos + offset;
                let cur_threads = queues.remove(&offset).unwrap_or_default();
                if cur_threads.is_empty() {
                    if queues.is_empty() {
                        break;
                    }
                    offset += 1;
                    continue;
                }
                let expanded = follow_nonconsuming(&cur_threads, nfa, cur_pos, &chars, flags);
                // Check for match at cur_pos (after epsilon closure)
                for t in &expanded {
                    if nfa.states[t.pc].is_match {
                        let m = build_groups(&t.saves, pos, cur_pos, nfa.num_captures);
                        let should_replace = match &longest {
                            None => true,
                            Some(prev) => cur_pos > prev.groups[0].1,
                        };
                        if should_replace {
                            longest = Some(m);
                        }
                    }
                }
                if cur_pos == chars.len() {
                    continue;
                }
                let c = chars[cur_pos];
                for t in &expanded {
                    for edge in &nfa.states[t.pc].edges {
                        match edge {
                            Edge::Char(ch, target) => {
                                let eq = if flags & 2 != 0 {
                                    ch.eq_ignore_ascii_case(&c)
                                } else {
                                    *ch == c
                                };
                                if eq {
                                    add_thread(
                                        queues.entry(offset + 1).or_default(),
                                        nfa,
                                        *target,
                                        &t.saves,
                                    );
                                }
                            }
                            Edge::CharClass {
                                negated,
                                ranges,
                                target,
                            } => {
                                let mut in_class =
                                    ranges.iter().any(|(lo, hi)| c >= *lo && c <= *hi);
                                if flags & 2 != 0 && !in_class {
                                    // Case-insensitive: also check lower/upper variants
                                    let lower = c.to_ascii_lowercase();
                                    let upper = c.to_ascii_uppercase();
                                    in_class = ranges.iter().any(|(lo, hi)| {
                                        lower >= *lo && lower <= *hi || upper >= *lo && upper <= *hi
                                    });
                                }
                                if *negated != in_class {
                                    add_thread(
                                        queues.entry(offset + 1).or_default(),
                                        nfa,
                                        *target,
                                        &t.saves,
                                    );
                                }
                            }
                            Edge::Dot(target) => {
                                if flags & 8 != 0 || c != '\n' {
                                    add_thread(
                                        queues.entry(offset + 1).or_default(),
                                        nfa,
                                        *target,
                                        &t.saves,
                                    );
                                }
                            }
                            Edge::Backref { index, target } => {
                                if *index == 0 {
                                    continue;
                                }
                                let s_idx = (*index - 1) * 2 + 2;
                                let e_idx = (*index - 1) * 2 + 3;
                                if s_idx >= t.saves.len() || e_idx >= t.saves.len() {
                                    continue;
                                }
                                match (t.saves[s_idx], t.saves[e_idx]) {
                                    (Some(s), Some(e)) => {
                                        let cap_len = e - s;
                                        if cap_len == 0 {
                                            // Empty — already handled as epsilon in follow, skip here
                                            continue;
                                        }
                                        if cur_pos + cap_len <= chars.len() {
                                            let mut ok = true;
                                            for k in 0..cap_len {
                                                let a = chars[s + k];
                                                let b = chars[cur_pos + k];
                                                let eq = if flags & 2 != 0 {
                                                    a.eq_ignore_ascii_case(&b)
                                                } else {
                                                    a == b
                                                };
                                                if !eq {
                                                    ok = false;
                                                    break;
                                                }
                                            }
                                            if ok {
                                                // Enqueue after consuming cap_len chars
                                                if offset + cap_len <= remaining {
                                                    add_thread(
                                                        queues.entry(offset + cap_len).or_default(),
                                                        nfa,
                                                        *target,
                                                        &t.saves,
                                                    );
                                                }
                                            }
                                        }
                                    }
                                    _ => {
                                        // Non-participating → empty, handled in follow
                                    }
                                }
                            }
                            Edge::BackrefMulti { indices, target } => {
                                for index in indices {
                                    let s_idx = (*index - 1) * 2 + 2;
                                    let e_idx = (*index - 1) * 2 + 3;
                                    if s_idx >= t.saves.len() || e_idx >= t.saves.len() {
                                        continue;
                                    }
                                    let (Some(s), Some(e)) = (t.saves[s_idx], t.saves[e_idx])
                                    else {
                                        continue;
                                    };
                                    let cap_len = e - s;
                                    if cap_len == 0 || cur_pos + cap_len > chars.len() {
                                        continue;
                                    }
                                    let mut ok = true;
                                    for k in 0..cap_len {
                                        let a = chars[s + k];
                                        let b = chars[cur_pos + k];
                                        let eq = if flags & 2 != 0 {
                                            a.eq_ignore_ascii_case(&b)
                                        } else {
                                            a == b
                                        };
                                        if !eq {
                                            ok = false;
                                            break;
                                        }
                                    }
                                    if ok {
                                        if offset + cap_len <= remaining {
                                            add_thread(
                                                queues.entry(offset + cap_len).or_default(),
                                                nfa,
                                                *target,
                                                &t.saves,
                                            );
                                        }
                                        break;
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                offset += 1;
            }
            if let Some(mut m) = longest {
                for group in m.groups.iter_mut() {
                    if group.0 != usize::MAX {
                        group.0 = char_bytes[group.0];
                        group.1 = char_bytes[group.1];
                    }
                }
                return Some(m);
            }
        }
        None
    }
}

/// Follow all non-consuming edges (Save, Epsilon, Lookahead) until fixpoint.
fn follow_nonconsuming(
    threads: &[Thread],
    nfa: &Nfa,
    pos: usize,
    chars: &[char],
    flags: u32,
) -> Vec<Thread> {
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
                    let new_t = Thread {
                        pc: *target,
                        saves: t.saves.clone(),
                    };
                    if !in_sets(&new_t, &worklist, &result) {
                        worklist.push(new_t);
                    }
                }
                Edge::Clear { slots, target } => {
                    has_nonconsuming = true;
                    let mut saves = t.saves.clone();
                    for slot in slots {
                        if *slot < saves.len() {
                            saves[*slot] = None;
                        }
                    }
                    let new_t = Thread { pc: *target, saves };
                    if !in_sets(&new_t, &worklist, &result) {
                        worklist.push(new_t);
                    }
                }
                Edge::Lookahead {
                    sub_start,
                    sub_match,
                    negated,
                    target,
                } => {
                    has_nonconsuming = true;
                    let sub_result =
                        lookahead_matches(nfa, *sub_start, *sub_match, chars, pos, &t.saves, flags);
                    let ok = if *negated {
                        sub_result.is_none()
                    } else {
                        sub_result.is_some()
                    };
                    if ok {
                        // Positive lookahead captures merge into the thread's
                        // saves (spec: captures inside (?=...) participate).
                        // lookahead_matches starts from the outer saves, so the
                        // returned saves already contain both.
                        let new_t = Thread {
                            pc: *target,
                            saves: sub_result.unwrap_or_else(|| t.saves.clone()),
                        };
                        if !in_sets(&new_t, &worklist, &result) {
                            worklist.push(new_t);
                        }
                    }
                }
                Edge::AnchorStart(target) => {
                    has_nonconsuming = true;
                    let is_start =
                        pos == 0 || (flags & 4 != 0 && pos > 0 && chars[pos - 1] == '\n');
                    if is_start {
                        let new_t = Thread {
                            pc: *target,
                            saves: t.saves.clone(),
                        };
                        if !in_sets(&new_t, &worklist, &result) {
                            worklist.push(new_t);
                        }
                    }
                }
                Edge::AnchorEnd(target) => {
                    has_nonconsuming = true;
                    let is_end = pos == chars.len()
                        || (flags & 4 != 0 && pos < chars.len() && chars[pos] == '\n');
                    if is_end {
                        let new_t = Thread {
                            pc: *target,
                            saves: t.saves.clone(),
                        };
                        if !in_sets(&new_t, &worklist, &result) {
                            worklist.push(new_t);
                        }
                    }
                }
                Edge::WordBoundary { negated, target } => {
                    has_nonconsuming = true;
                    let left_is_word = if pos == 0 {
                        false
                    } else {
                        is_word_char(chars[pos - 1])
                    };
                    let right_is_word = if pos >= chars.len() {
                        false
                    } else {
                        is_word_char(chars[pos])
                    };
                    let is_boundary = left_is_word != right_is_word;
                    let ok = if *negated { !is_boundary } else { is_boundary };
                    if ok {
                        let new_t = Thread {
                            pc: *target,
                            saves: t.saves.clone(),
                        };
                        if !in_sets(&new_t, &worklist, &result) {
                            worklist.push(new_t);
                        }
                    }
                }
                Edge::Backref { index, target } => {
                    // Empty or non-participating backref acts as epsilon (zero-width)
                    // at same position; non-empty is handled as consuming in the main loop.
                    if *index == 0 {
                        has_nonconsuming = true;
                        continue;
                    }
                    let s_idx = (*index - 1) * 2 + 2;
                    let e_idx = (*index - 1) * 2 + 3;
                    if s_idx >= t.saves.len() || e_idx >= t.saves.len() {
                        has_nonconsuming = true;
                        continue;
                    }
                    match (t.saves[s_idx], t.saves[e_idx]) {
                        (Some(s), Some(e)) if e != s => {
                            // Non-empty: consuming, leave for main loop — do not mark as non-consuming
                        }
                        _ => {
                            // Empty or non-participating → epsilon
                            has_nonconsuming = true;
                            let new_t = Thread {
                                pc: *target,
                                saves: t.saves.clone(),
                            };
                            if !in_sets(&new_t, &worklist, &result) {
                                worklist.push(new_t);
                            }
                        }
                    }
                }
                Edge::BackrefMulti { indices, target } => {
                    let mut nonempty = false;
                    for index in indices {
                        let s_idx = (*index - 1) * 2 + 2;
                        let e_idx = (*index - 1) * 2 + 3;
                        if s_idx >= t.saves.len() || e_idx >= t.saves.len() {
                            continue;
                        }
                        if let (Some(s), Some(e)) = (t.saves[s_idx], t.saves[e_idx]) {
                            if e != s {
                                nonempty = true;
                                break;
                            }
                        }
                    }
                    if !nonempty {
                        has_nonconsuming = true;
                        let new_t = Thread {
                            pc: *target,
                            saves: t.saves.clone(),
                        };
                        if !in_sets(&new_t, &worklist, &result) {
                            worklist.push(new_t);
                        }
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

/// Run the lookahead sub-automaton rooted at `sub_start` from position `pos`
/// over `chars`. Returns the saves of the longest sub-match (initialized from
/// the outer thread's saves so outer captures survive and inner captures
/// overwrite their own slots), or None if the sub-pattern does not match.
fn lookahead_matches(
    nfa: &Nfa,
    sub_start: usize,
    sub_match: usize,
    chars: &[char],
    pos: usize,
    init_saves: &[Option<usize>],
    flags: u32,
) -> Option<Vec<Option<usize>>> {
    let mut best: Option<(usize, Vec<Option<usize>>)> = None;
    let mut clist: Vec<Thread> = Vec::new();
    add_thread(&mut clist, nfa, sub_start, init_saves);
    let mut p = pos;
    loop {
        let expanded = follow_nonconsuming(&clist, nfa, p, chars, flags);
        for t in &expanded {
            if t.pc == sub_match {
                let should_replace = match &best {
                    None => true,
                    Some((prev_end, _)) => p > *prev_end,
                };
                if should_replace {
                    best = Some((p, t.saves.clone()));
                }
            }
        }
        if p >= chars.len() || clist.is_empty() {
            break;
        }
        let c = chars[p];
        let mut nlist: Vec<Thread> = Vec::new();
        for t in &expanded {
            for edge in &nfa.states[t.pc].edges {
                match edge {
                    Edge::Char(ch, target) => {
                        let eq = if flags & 2 != 0 {
                            ch.eq_ignore_ascii_case(&c)
                        } else {
                            *ch == c
                        };
                        if eq {
                            add_thread(&mut nlist, nfa, *target, &t.saves);
                        }
                    }
                    Edge::CharClass {
                        negated,
                        ranges,
                        target,
                    } => {
                        let mut in_class = ranges.iter().any(|(lo, hi)| c >= *lo && c <= *hi);
                        if flags & 2 != 0 && !in_class {
                            let lower = c.to_ascii_lowercase();
                            let upper = c.to_ascii_uppercase();
                            in_class = ranges.iter().any(|(lo, hi)| {
                                lower >= *lo && lower <= *hi || upper >= *lo && upper <= *hi
                            });
                        }
                        if *negated != in_class {
                            add_thread(&mut nlist, nfa, *target, &t.saves);
                        }
                    }
                    Edge::Dot(target) => {
                        if flags & 8 != 0 || c != '\n' {
                            add_thread(&mut nlist, nfa, *target, &t.saves);
                        }
                    }
                    Edge::Epsilon(_)
                    | Edge::Save(_, _)
                    | Edge::Lookahead { .. }
                    | Edge::AnchorStart(_)
                    | Edge::AnchorEnd(_)
                    | Edge::WordBoundary { .. }
                    | Edge::Backref { .. }
                    | Edge::BackrefMulti { .. }
                    | Edge::Clear { .. } => {}
                }
            }
        }
        clist = nlist;
        p += 1;
    }
    best.map(|(_, saves)| saves)
}

fn in_result(t: &Thread, result: &[Thread]) -> bool {
    result.iter().any(|r| r.pc == t.pc && r.saves == t.saves)
}

fn build_groups(
    saves: &[Option<usize>],
    match_start: usize,
    match_end: usize,
    num_captures: usize,
) -> Match {
    let mut groups = vec![(0, 0); num_captures + 1];
    groups[0] = (saves[0].unwrap_or(match_start), match_end);
    for i in 0..num_captures {
        let s = i * 2 + 2;
        let e = i * 2 + 3;
        groups[i + 1] = match (saves[s], saves[e]) {
            (Some(gs), Some(ge)) => (gs, ge),
            _ => (usize::MAX, usize::MAX),
        };
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
    nlist.push(Thread {
        pc,
        saves: saves.to_vec(),
    });
    for edge in &nfa.states[pc].edges {
        match edge {
            Edge::Epsilon(target) => add_thread(nlist, nfa, *target, saves),
            Edge::Clear { slots, target } => {
                let mut saves = saves.to_vec();
                for slot in slots {
                    if *slot < saves.len() {
                        saves[*slot] = None;
                    }
                }
                add_thread(nlist, nfa, *target, &saves);
            }
            _ => {}
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

    #[test]
    fn test_positive_lookahead() {
        let expr = parse_regex(r"a(?=b)").unwrap();
        let nfa = crate::nfa::compile(&expr);
        let vm = PikeVm::new();
        let m = vm.exec(&nfa, "abac", 0).unwrap();
        assert_eq!(m.groups[0], (0, 1));
        assert_eq!(vm.exec(&nfa, "ac", 0), None);
    }

    #[test]
    fn test_lookahead_with_capture() {
        // Backrefs (\1) are no-ops in this engine, so use a plain `a` after
        // the lookahead; the lookahead's capture must still be recorded.
        let expr = parse_regex(r"(?=(a+))a").unwrap();
        let nfa = crate::nfa::compile(&expr);
        assert_eq!(nfa.num_captures, 1);
        let vm = PikeVm::new();
        let m = vm.exec(&nfa, "baaaab", 0).unwrap();
        assert_eq!(m.groups[0], (1, 2));
        assert_eq!(m.groups[1], (1, 5));
    }

    #[test]
    fn test_negative_lookahead() {
        let expr = parse_regex(r"a(?!b)").unwrap();
        let nfa = crate::nfa::compile(&expr);
        let vm = PikeVm::new();
        let m = vm.exec(&nfa, "ac", 0).unwrap();
        assert_eq!(m.groups[0], (0, 1));
        assert_eq!(vm.exec(&nfa, "ab", 0), None);
    }

    #[test]
    fn test_lookahead_after_match_pos() {
        let expr = parse_regex(r"(?=\d{3}$)\d{3}").unwrap();
        let nfa = crate::nfa::compile(&expr);
        let vm = PikeVm::new();
        let m = vm.exec(&nfa, "abc123", 0).unwrap();
        assert_eq!(m.groups[0], (3, 6));
    }
}
