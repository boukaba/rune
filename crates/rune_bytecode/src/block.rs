use crate::opcode::{Instruction, Opcode};

/// A basic block in the CFG — a straight-line sequence of instructions
/// with a single entry point and a single exit point.
#[derive(Clone, Debug)]
pub struct BasicBlock {
    pub id: usize,
    /// Index of first instruction in this block (inclusive).
    pub start: usize,
    /// Index past the last instruction (exclusive).
    pub end: usize,
    /// Indices of successor blocks.
    pub successors: Vec<usize>,
    /// Indices of predecessor blocks.
    pub predecessors: Vec<usize>,
}

impl BasicBlock {
    pub fn new(id: usize, start: usize) -> Self {
        BasicBlock {
            id,
            start,
            end: start,
            successors: vec![],
            predecessors: vec![],
        }
    }
}

/// A control-flow graph over a flat instruction list.
#[derive(Clone, Debug)]
pub struct ControlFlowGraph {
    pub blocks: Vec<BasicBlock>,
    /// Index of the entry block (always 0).
    pub entry: usize,
}

/// Identify which opcodes terminate a basic block.
fn is_terminator(op: Opcode) -> bool {
    matches!(
        op,
        Opcode::Jump
            | Opcode::JumpIfTrue
            | Opcode::JumpIfFalse
            | Opcode::Return
            | Opcode::Throw
            | Opcode::ForInNext
    )
}

/// Return the jump target instruction index, if this is a branching opcode.
fn jump_target(instr: &Instruction) -> Option<usize> {
    match instr.opcode {
        Opcode::Jump | Opcode::JumpIfTrue | Opcode::JumpIfFalse | Opcode::ForInNext => {
            Some(instr.operands[0] as usize)
        }
        _ => None,
    }
}

/// Build a CFG from a flat list of bytecode instructions.
///
/// Algorithm:
/// 1. Find *leaders*: instruction 0, targets of branches, and instructions
///    immediately after terminators.
/// 2. Partition instructions into blocks between consecutive leaders.
/// 3. Compute successor / predecessor edges.
///
/// Exception edges (try/catch) are not represented in this CFG.
pub fn build_cfg(instrs: &[Instruction]) -> ControlFlowGraph {
    if instrs.is_empty() {
        return ControlFlowGraph {
            blocks: vec![],
            entry: 0,
        };
    }

    // Phase 1: find leaders
    let n = instrs.len();
    let mut is_leader = vec![false; n];
    is_leader[0] = true;

    for (i, instr) in instrs.iter().enumerate() {
        if is_terminator(instr.opcode) {
            // Instruction after a terminator is a leader (if it exists)
            if i + 1 < n {
                is_leader[i + 1] = true;
            }
        }
        // Jump targets are leaders
        if let Some(target) = jump_target(instr)
            && target < n
        {
            is_leader[target] = true;
        }
    }

    // Phase 2: build blocks
    let mut blocks: Vec<BasicBlock> = vec![];
    let mut i = 0;
    while i < n {
        let block_id = blocks.len();
        let mut block = BasicBlock::new(block_id, i);
        i += 1;
        while i < n && !is_leader[i] {
            i += 1;
        }
        block.end = i;
        blocks.push(block);
    }

    // Map instruction index → block index
    let mut instr_to_block: Vec<usize> = vec![0; n];
    for b in &blocks {
        for slot in instr_to_block[b.start..b.end].iter_mut() {
            *slot = b.id;
        }
    }

    // Phase 3: compute edges
    for b_idx in 0..blocks.len() {
        let block = &blocks[b_idx];
        let last_idx = block.end.saturating_sub(1);
        if block.start > block.end {
            continue;
        }
        let last_instr = &instrs[last_idx];

        match last_instr.opcode {
            Opcode::Jump => {
                // Unconditional: one successor
                let target = last_instr.operands[0] as usize;
                let target_block = instr_to_block[target];
                blocks[b_idx].successors.push(target_block);
            }
            Opcode::JumpIfTrue | Opcode::JumpIfFalse | Opcode::ForInNext => {
                // Conditional: two successors (taken + fall-through)
                let target = last_instr.operands[0] as usize;
                let target_block = instr_to_block[target];
                blocks[b_idx].successors.push(target_block);
                // Fall-through: next block
                if b_idx + 1 < blocks.len() {
                    blocks[b_idx].successors.push(b_idx + 1);
                }
            }
            Opcode::Return | Opcode::Throw => {
                // No successors
            }
            _ => {
                // Fall-through to next block
                if b_idx + 1 < blocks.len() {
                    blocks[b_idx].successors.push(b_idx + 1);
                }
            }
        }
    }

    // Populate predecessors from successors
    for b_idx in 0..blocks.len() {
        for &succ in blocks[b_idx].successors.clone().iter() {
            blocks[succ].predecessors.push(b_idx);
        }
    }

    ControlFlowGraph { blocks, entry: 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opcode::{Instruction, Opcode};

    #[test]
    fn test_cfg_linear() {
        // LoadSmi(1), LoadSmi(2), Add, Return — single block
        let instrs = vec![
            Instruction::new(Opcode::LoadSmi, vec![1]),
            Instruction::new(Opcode::LoadSmi, vec![2]),
            Instruction::new(Opcode::Add, vec![]),
            Instruction::new(Opcode::Return, vec![]),
        ];
        let cfg = build_cfg(&instrs);
        assert_eq!(cfg.blocks.len(), 1);
        assert_eq!(cfg.blocks[0].start, 0);
        assert_eq!(cfg.blocks[0].end, 4);
        assert!(cfg.blocks[0].successors.is_empty());
    }

    #[test]
    fn test_cfg_if_else() {
        // JumpIfFalse else_target; ..., Jump end_target; else: ...; end: Return
        let instrs = vec![
            Instruction::new(Opcode::LoadSmi, vec![0]),
            Instruction::new(Opcode::JumpIfFalse, vec![4]), // jump to idx 4 if false
            Instruction::new(Opcode::LoadSmi, vec![1]),     // then branch
            Instruction::new(Opcode::Jump, vec![6]),        // jump to idx 6
            Instruction::new(Opcode::LoadSmi, vec![2]),     // else branch (idx 4)
            Instruction::new(Opcode::LoadSmi, vec![3]),     // (idx 5)
            Instruction::new(Opcode::Return, vec![]),       // end (idx 6)
        ];
        let cfg = build_cfg(&instrs);
        // Expected blocks:
        //   B0: [0..2)  — after JumpIfFalse (is terminator) leader at 2
        //        fallen into B1 (idx 1) or jumps to B2 (idx 4)
        //   B1: [2..4)  — after Jump (is terminator) leader at 4
        //        fallen into B3 (idx 3)
        //   B2: [4..6)  — after last instr (LoadSmi at idx 5, not terminator)
        //        fallen into B3
        //   B3: [6..7)  — Return, no successors
        assert_eq!(cfg.blocks.len(), 4);
        // B0 successors: B2 (target=4) and B1 (fall-through=idx 2)
        assert!(cfg.blocks[0].successors.contains(&1));
        assert!(cfg.blocks[0].successors.contains(&2));
        // B1 successors: B3 (target=6)
        assert_eq!(cfg.blocks[1].successors, vec![3]);
        // B2 successors: B3 (fall-through)
        assert_eq!(cfg.blocks[2].successors, vec![3]);
        // B3: no successors
        assert!(cfg.blocks[3].successors.is_empty());
    }

    #[test]
    fn test_cfg_loop() {
        // loop: idx=0: ...; idx=1: JumpIfFalse end; idx=2: ...; idx=3: Jump loop; end: ...
        let instrs = vec![
            Instruction::new(Opcode::LoadSmi, vec![0]),     // idx 0
            Instruction::new(Opcode::JumpIfFalse, vec![4]), // idx 1 — jump to 4 if false
            Instruction::new(Opcode::LoadSmi, vec![1]),     // idx 2 — loop body
            Instruction::new(Opcode::Jump, vec![0]),        // idx 3 — back to loop start
            Instruction::new(Opcode::Return, vec![]),       // idx 4 — end
        ];
        let cfg = build_cfg(&instrs);
        // Expected:
        //   B0: [0..2) — JumpIfFalse: succ=end(B2), fall-through B1
        //   B1: [2..4) — Jump to B0: succ=B0
        //   B2: [4..5) — Return: no succ
        assert_eq!(cfg.blocks.len(), 3);
        assert!(cfg.blocks[0].successors.contains(&2)); // end
        assert!(cfg.blocks[0].successors.contains(&1)); // body
        assert_eq!(cfg.blocks[1].successors, vec![0]); // back-edge
        assert!(cfg.blocks[2].successors.is_empty());
    }

    #[test]
    fn test_cfg_for_in_next() {
        // ForInNext is a conditional branch
        let instrs = vec![
            Instruction::new(Opcode::ForInNext, vec![3]), // idx 0 — jump to 3 if done
            Instruction::new(Opcode::StoreLocal, vec![0]), // idx 1 — body
            Instruction::new(Opcode::Jump, vec![0]),      // idx 2 — back to ForInNext
            Instruction::new(Opcode::Return, vec![]),     // idx 3 — after loop
        ];
        let cfg = build_cfg(&instrs);
        assert_eq!(cfg.blocks.len(), 3);
        // B0: succs = B2 (target=3) and B1 (fall-through)
        assert!(cfg.blocks[0].successors.contains(&2));
        assert!(cfg.blocks[0].successors.contains(&1));
        // B1: succs = B0 (Jump back)
        assert_eq!(cfg.blocks[1].successors, vec![0]);
        // B2: no succ (Return)
        assert!(cfg.blocks[2].successors.is_empty());
    }
}
