fn main() {
    let mut ctx = rune_embed::Context::new_small();
    let val = ctx.eval("var {a, b = 99} = {a: 10}; a + b").unwrap();
    println!("a + b = {:?}", val);
    assert_eq!(val.as_smi(), Some(109));
}
