use rune_embed::Context;

fn main() {
    let src = std::env::args().nth(1).unwrap_or_default();
    let ctx = Context::new_small();
    let prog = ctx.compile(&src).expect("compile");
    fn dump(prog: &rune_bytecode::opcode::BytecodeProgram, indent: usize) {
        for (i, instr) in prog.instructions.iter().enumerate() {
            println!(
                "{:indent$}{:4}: {:?} {:?}",
                "",
                i,
                instr.opcode,
                instr.operands,
                indent = indent
            );
        }
        for f in &prog.functions {
            println!("{:indent$}---- function ----", "", indent = indent);
            dump(f, indent + 4);
        }
    }
    dump(&prog, 0);
}
