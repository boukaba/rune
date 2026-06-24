/// Demonstrates the AFPC snapshot cache: first run saves, subsequent runs skip parse+emit
fn main() {
    let source = "var s = 0; for (var i = 0; i < 100000; i++) s += i; s";
    let mut ctx = rune_embed::Context::new_small();
    let val = ctx.eval(source).unwrap();
    println!("Sum: {:?}", val);
}
