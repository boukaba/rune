/// Liveness analysis and escape analysis.

#[derive(Default)]
pub struct Analysis;

impl Analysis {
    pub fn new() -> Self {
        Analysis
    }

    pub fn liveness(&self, _instrs: &[crate::opcode::Instruction]) -> Vec<Vec<usize>> {
        vec![]
    }

    pub fn escape_analysis(&self, _instrs: &[crate::opcode::Instruction]) -> Vec<bool> {
        vec![]
    }
}
