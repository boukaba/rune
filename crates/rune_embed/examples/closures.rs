fn main() {
    let mut ctx = rune_embed::Context::new_small();
    // Counter closure: the feature that took 6 days to land
    let val = ctx.eval(
        "function counter() { var c = 0; return function() { c = c + 1; return c; }; } var cc = counter(); cc(); cc(); cc()"
    ).unwrap();
    println!("counter()()() => {:?}", val);
    assert_eq!(val.as_smi(), Some(3));
}
