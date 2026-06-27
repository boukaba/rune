use crate::block::ControlFlowGraph;
use crate::opcode::{Instruction, Opcode};
use std::collections::HashSet;

/// Per-block liveness information: which local variables are live
/// at the entry and exit of each basic block.
#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct LivenessInfo {
    pub live_in: Vec<HashSet<usize>>,
    pub live_out: Vec<HashSet<usize>>,
}

/// Build the `use` and `def` sets for each basic block.
///
/// - `def[b]` = set of local variable indices that are **written** (StoreLocal)
///   before any read in block b.
/// - `use[b]` = set of local variable indices that are **read** (LoadLocal)
///   before any write in block b.
fn compute_use_def(
    blocks: &[crate::block::BasicBlock],
    instrs: &[Instruction],
) -> (Vec<HashSet<usize>>, Vec<HashSet<usize>>) {
    let n = blocks.len();
    let mut use_set: Vec<HashSet<usize>> = vec![HashSet::new(); n];
    let mut def_set: Vec<HashSet<usize>> = vec![HashSet::new(); n];

    for (b_idx, block) in blocks.iter().enumerate() {
        let mut defd = HashSet::new();
        for i in block.start..block.end {
            if i >= instrs.len() {
                break;
            }
            match instrs[i].opcode {
                Opcode::LoadLocal => {
                    let idx = instrs[i].operands[0] as usize;
                    if !defd.contains(&idx) {
                        use_set[b_idx].insert(idx);
                    }
                }
                Opcode::StoreLocal => {
                    let idx = instrs[i].operands[0] as usize;
                    defd.insert(idx);
                    def_set[b_idx].insert(idx);
                }
                Opcode::DeclareLet | Opcode::DeclareConst => {
                    let idx = instrs[i].operands[0] as usize;
                    defd.insert(idx);
                    def_set[b_idx].insert(idx);
                }
                Opcode::LoadLexical => {
                    let idx = instrs[i].operands[0] as usize;
                    if !defd.contains(&idx) {
                        use_set[b_idx].insert(idx);
                    }
                }
                Opcode::StoreLexical => {
                    let idx = instrs[i].operands[0] as usize;
                    defd.insert(idx);
                    def_set[b_idx].insert(idx);
                }
                _ => {}
            }
        }
    }

    (use_set, def_set)
}

/// Perform liveness analysis on a CFG using iterative dataflow.
///
/// Local variables are tracked by their index in the `locals` array.
/// The analysis converges because each iteration only adds elements to
/// `live_in` / `live_out` sets, bounded by `local_count`.
///
/// Returns live_in and live_out per block (indexed by block ID).
pub fn liveness(
    cfg: &ControlFlowGraph,
    instrs: &[Instruction],
    _local_count: usize,
) -> LivenessInfo {
    let n = cfg.blocks.len();
    if n == 0 {
        return LivenessInfo {
            live_in: vec![],
            live_out: vec![],
        };
    }

    let (use_set, def_set) = compute_use_def(&cfg.blocks, instrs);

    let mut live_in: Vec<HashSet<usize>> = vec![HashSet::new(); n];
    let mut live_out: Vec<HashSet<usize>> = vec![HashSet::new(); n];

    // Iterate until fixed point
    loop {
        let mut changed = false;

        for b_idx in (0..n).rev() {
            // live_out[b] = ∪ live_in[s] for all successors s
            let mut new_out = HashSet::new();
            for &succ in &cfg.blocks[b_idx].successors {
                new_out.extend(&live_in[succ]);
            }

            // live_in[b] = use[b] ∪ (live_out[b] - def[b])
            let mut new_in = use_set[b_idx].clone();
            for v in new_out.difference(&def_set[b_idx]) {
                new_in.insert(*v);
            }

            if new_in != live_in[b_idx] || new_out != live_out[b_idx] {
                changed = true;
                live_in[b_idx] = new_in;
                live_out[b_idx] = new_out;
            }
        }

        if !changed {
            break;
        }
    }

    LivenessInfo { live_in, live_out }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::build_cfg;
    use crate::opcode::{Instruction, Opcode};

    #[test]
    fn test_liveness_multi_block() {
        // B0: i = 1; if (cond) Jump B1 else Jump B2
        // B1: i = i + 1; Jump B2
        // B2: return i;
        let instrs = vec![
            Instruction::new(Opcode::LoadSmi, vec![1]),     // idx 0
            Instruction::new(Opcode::StoreLocal, vec![0]),  // idx 1 — i = 1 (def 0)
            Instruction::new(Opcode::Pop, vec![]),          // idx 2
            Instruction::new(Opcode::LoadSmi, vec![1]),     // idx 3 — cond = true
            Instruction::new(Opcode::JumpIfFalse, vec![7]), // idx 4 — if !cond goto B2
            Instruction::new(Opcode::LoadLocal, vec![0]),   // idx 5 — use 0
            Instruction::new(Opcode::LoadSmi, vec![1]),     // idx 6
            Instruction::new(Opcode::Add, vec![]),          // idx 7
            Instruction::new(Opcode::StoreLocal, vec![0]),  // idx 8 — def 0 (i = i+1)
            Instruction::new(Opcode::Pop, vec![]),          // idx 9
            Instruction::new(Opcode::Jump, vec![11]),       // idx 10 — goto join
            Instruction::new(Opcode::LoadLocal, vec![0]),   // idx 11 — use 0 (join block)
            Instruction::new(Opcode::Return, vec![]),       // idx 12
        ];
        let cfg = build_cfg(&instrs);
        let info = liveness(&cfg, &instrs, 1);

        // i is used in both B1 (idx 5) and join block (idx 11),
        // so it should be live at exit of B0 and at entry of B1 and join.
        let join_block = cfg.blocks.iter().find(|b| b.start == 11).unwrap();
        assert!(
            info.live_in[join_block.id].contains(&0),
            "i live at join entry"
        );
    }

    #[test]
    fn test_liveness_loop() {
        // var i = 0; loop { i = i + 1; if (i < 10) continue; break; }
        // Simplified: StoreLocal 0; loop: LoadLocal 0; Add; StoreLocal 0;
        //            LoadLocal 0; LoadSmi 10; Lt; JumpIfFalse end; Jump loop; end: Return
        let instrs = vec![
            Instruction::new(Opcode::LoadSmi, vec![0]),
            Instruction::new(Opcode::StoreLocal, vec![0]), // i=0 (def 0)
            Instruction::new(Opcode::Pop, vec![]),
            // loop header
            Instruction::new(Opcode::LoadLocal, vec![0]), // use 0
            Instruction::new(Opcode::LoadSmi, vec![1]),
            Instruction::new(Opcode::Add, vec![]),
            Instruction::new(Opcode::StoreLocal, vec![0]), // def 0
            Instruction::new(Opcode::Pop, vec![]),
            Instruction::new(Opcode::LoadLocal, vec![0]), // use 0
            Instruction::new(Opcode::LoadSmi, vec![10]),
            Instruction::new(Opcode::Lt, vec![]),
            Instruction::new(Opcode::JumpIfFalse, vec![13]), // to end
            Instruction::new(Opcode::Jump, vec![3]),         // back to loop
            Instruction::new(Opcode::Return, vec![]),        // end
        ];
        let cfg = build_cfg(&instrs);
        let info = liveness(&cfg, &instrs, 1);
        // B0: [0..3) — init block, succ = B1
        // B1: [3..12) — loop body, succ = B1 (back edge) + B2 (end)
        // B2: [12..13) — jump to loop (B1)
        // B3: [13..14) — return

        // i should be live at entry of loop body
        assert!(
            info.live_in[1].contains(&0),
            "i should be live at entry of loop body"
        );
        // i should be live at exit of init block (B0 → B1 needs it)
        assert!(
            info.live_out[0].contains(&0),
            "i should be live at exit of init block"
        );
        // i should be live at exit of loop body (back-edge to itself)
        assert!(
            info.live_out[1].contains(&0),
            "i should be live at exit of loop body"
        );
    }
}
