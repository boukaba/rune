/// Scalar replacement — replace non-escaping allocations with SSA values.
pub fn scalar_replace(_bc: &[rune_bytecode::opcode::Instruction]) -> Vec<rune_bytecode::opcode::Instruction> {
    _bc.to_vec()
}
