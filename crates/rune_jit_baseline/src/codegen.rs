/// Bytecode-to-machine-code compiler (copy-and-patch).

pub struct Codegen;

impl Codegen {
    pub fn new() -> Self {
        Codegen
    }

    pub fn compile(&self, _bytecode: &[rune_bytecode::opcode::Instruction]) -> Option<*const u8> {
        None
    }
}
