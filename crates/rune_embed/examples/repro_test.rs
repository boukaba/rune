use rune_embed::Context;
fn main() {
    let mut ctx = Context::new_small();
    let code = r#"function f(n) { let acc = 0; for (let i = 0; i < n; i++) { acc += i * i; } return acc; } f(70000)"#;
    match ctx.eval(code) {
        Ok(v) => println!("ok: {v:?}"),
        Err(e) => println!("err: {e}"),
    }
}
