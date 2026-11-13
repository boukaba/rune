//! JIT-on/off differential checks. Native code must produce the same observable
//! result as the interpreter for this hot-path corpus. Gated to the primary
//! AArch64 JIT target; x86-64 codegen remains opt-in and less mature.
#![cfg(target_arch = "aarch64")]

use rune_embed::Context;
use rune_interpreter::builtins::value_to_js_string;

fn run(enable_jit: bool, source: &str) -> String {
    let mut ctx = Context::new();
    ctx.enable_jit = enable_jit;
    match ctx.eval(source) {
        Ok(value) => value_to_js_string(value),
        Err(err) => format!("ERR:{err}"),
    }
}

#[test]
fn jit_matches_interpreter_on_hot_corpus() {
    let scripts = [
        "function f(n){var s=0;for(var i=0;i<n;i++)s+=i;return s;} f(2000);",
        "function fib(n){return n<2?n:fib(n-1)+fib(n-2);} fib(18);",
        "function f(o){return o.a+o.b;} var o={a:1,b:2}; var s=0; for(var i=0;i<500;i++)s+=f(o); s;",
        "function f(o,i){o.x=i;return o.x;} var o={}; var s=0; for(var i=0;i<500;i++)s+=f(o,i); s;",
        "function f(n){var s='';for(var i=0;i<n;i++)s+='x';return s.length;} f(1000);",
        "function mk(x){return function(y){return x+y;};} var f=mk(5); var s=0; for(var i=0;i<500;i++)s+=f(i); s;",
        "function f(a){var s=0;for(var i=0;i<a.length;i++)s+=a[i];return s;} var a=[]; for(var i=0;i<200;i++)a.push(i); f(a);",
        "var o={v:2,add:function(x){return this.v+x;}}; var s=0; for(var i=0;i<500;i++)s+=o.add(i); s;",
        "function f(n){try{if(n>0)throw 'x';return 1;}catch(e){return 2;}finally{}} var s=0; for(var i=0;i<200;i++)s+=f(i%2); s;",
        "function f(a,b){return (a*3+b)%7;} var s=0; for(var i=0;i<1000;i++)s+=f(i,i+1); s;",
        "function f(n){var s=0;for(var i=0;i<n;i++){if(i%3===0)s+=i;else s-=1;}return s;} f(1500);",
        "function f(i){if(i%3===0)return i;return -1;} var s=0; for(var k=0;k<500;k++)s+=f(k); s;",
        "function f(n){var s=0;var i=0;while(i<n){if(i>100)s+=i;i++;}return s;} f(300);",
    ];
    for (i, script) in scripts.iter().enumerate() {
        let interpreted = run(false, script);
        let native = run(true, script);
        assert_eq!(
            native, interpreted,
            "JIT differential mismatch on corpus item {i}:\n{script}"
        );
    }
}
