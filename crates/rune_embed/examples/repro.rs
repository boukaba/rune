use rune_embed::Context;

fn main() {
    let src = std::env::args().nth(1).unwrap_or_else(|| {
        "function f(n) { let total = 0; const inc = (v) => { total += v; }; for (let i = 0; i < n; i++) { inc(i * 2); } return total; } f(100)".to_string()
    });
    let mut ctx = Context::new_small();
    println!(
        "gc={:p} vm={:p} keepalive={:p} len={}",
        ctx.debug_gc_addr(),
        ctx.debug_vm_addr(),
        ctx.debug_keep_alive_addr(),
        ctx.debug_keep_alive_len()
    );
    let r = ctx.eval(&src).unwrap();
    println!("after eval: keepalive len={}", ctx.debug_keep_alive_len());
    println!("r = {:?}", r);
    println!("OK");
}
