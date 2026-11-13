//! Thread-local cache of parsed/compiled regex programs.
//!
//! The test262 RegExp suites execute the same patterns millions of times;
//! reparsing and recompiling per call dominates runtime. The cache is
//! thread-local (NFA is !Send) and bounded.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::nfa::{Nfa, compile};
use crate::parse::parse_regex;
use crate::pikevm::{Match, PikeVm};

pub struct Program {
    pub nfa: Nfa,
    pub named: Vec<(String, usize)>,
}

impl Program {
    pub fn exec(&self, input: &str, start: usize, flags: u32) -> Option<Match> {
        let vm = PikeVm::new();
        vm.exec_with_flags(&self.nfa, input, start, flags)
    }
}

thread_local! {
    static CACHE: RefCell<HashMap<String, Rc<Program>>> = RefCell::new(HashMap::new());
}

pub fn program(pattern: &str) -> Option<Rc<Program>> {
    CACHE.with(|cache| {
        if let Some(p) = cache.borrow().get(pattern) {
            return Some(p.clone());
        }
        let expr = parse_regex(pattern).ok()?;
        let named = crate::ast::named_captures(&expr);
        let nfa = compile(&expr);
        let program = Rc::new(Program { nfa, named });
        let mut map = cache.borrow_mut();
        if map.len() >= 4096 {
            map.clear();
        }
        map.insert(pattern.to_string(), program.clone());
        Some(program)
    })
}

pub fn exec(pattern: &str, flags: u32, input: &str, start: usize) -> Option<Match> {
    program(pattern)?.exec(input, start, flags)
}

pub fn named_captures(pattern: &str) -> Vec<(String, usize)> {
    program(pattern)
        .map(|p| p.named.clone())
        .unwrap_or_default()
}
