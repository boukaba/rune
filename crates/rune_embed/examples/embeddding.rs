/// Run `cargo run --example embedding` to test.
fn main() {
    let mut ctx = rune_embed::Context::new_small();
    let val = ctx
        .eval("var x = 1; function inc() { return x = x + 1; } inc() + inc()")
        .unwrap();
    println!("Result: {:?}", val);
    assert_eq!(val.as_smi(), Some(5));
}
