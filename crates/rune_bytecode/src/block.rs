use crate::opcode::Instruction;

/// A basic block in the CFG.
#[derive(Clone, Debug)]
pub struct BasicBlock {
    pub id: usize,
    pub instructions: Vec<Instruction>,
    pub successors: Vec<usize>,
    pub predecessors: Vec<usize>,
}

impl BasicBlock {
    pub fn new(id: usize) -> Self {
        BasicBlock {
            id,
            instructions: Vec::new(),
            successors: Vec::new(),
            predecessors: Vec::new(),
        }
    }
}
