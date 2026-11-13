use rune_embed::Context;

fn eval_smi(ctx: &mut Context, source: &str) -> i32 {
    ctx.eval(source).unwrap().as_smi().expect("smi result")
}

#[test]
fn test_gc_stress_method_arrow_nested_map_and_growth() {
    let mut ctx = Context::new_small();
    ctx.enable_gc_stress(64);
    let r = eval_smi(
        &mut ctx,
        r#"
        var o = {
            v: 7,
            m: function () {
                var f = () => this.v;
                var arr = [1, 2, 3].map(function (x) { return x + 1; });
                var g = [1];
                var holder = [g];
                for (var i = 0; i < 20; i++) { g.push(i); }
                for (var i = 0; i < 1000; i++) { var t = { n: i, s: "z" + i }; }
                return f() + arr[2] + holder[0].length;
            }
        };
        o.m();
        "#,
    );
    assert_eq!(r, 7 + 4 + 21);
}

#[test]
fn test_gc_stress_object_aliases_survive_growth() {
    let mut ctx = Context::new_small();
    ctx.enable_gc_stress(48);
    let r = eval_smi(
        &mut ctx,
        r#"
        var o = { x: 42 };
        var holder = [o];
        for (var i = 0; i < 30; i++) { o["p" + i] = i; }
        for (var i = 0; i < 500; i++) { var t = { n: i, s: "z" + i }; }
        holder[0].x;
        "#,
    );
    assert_eq!(r, 42);
}

#[test]
fn test_gc_stress_promise_all_and_pending_rejection() {
    let mut ctx = Context::new_small();
    ctx.enable_gc_stress(48);
    ctx.eval(
        r#"
        var out = "";
        var r1, r2;
        var p1 = new Promise(function (r) { r1 = r; });
        var p2 = new Promise(function (r) { r2 = r; });
        Promise.all([p1, p2]).then(function (v) { out = v.join(","); });
        for (var i = 0; i < 500; i++) { var t = { n: i, s: "z" + i }; }
        r2(2);
        r1(1);
        0;
        "#,
    )
    .unwrap();
    let v = ctx.eval("out").unwrap();
    let ptr = v.heap_ptr().unwrap();
    let out = unsafe {
        rune_core::string::HeapString::to_string(ptr as *mut rune_core::string::HeapString)
    };
    assert_eq!(out, "1,2");
}

#[test]
fn test_gc_stress_async_await_resumes() {
    let mut ctx = Context::new_small();
    ctx.enable_gc_stress(64);
    ctx.eval(
        r#"
        var out = 0;
        async function f() {
            var s = 0;
            for (var i = 0; i < 3; i++) {
                s += await Promise.resolve(i);
                for (var j = 0; j < 100; j++) { var t = { n: j, s: "z" + j }; }
            }
            out = s;
        }
        f();
        0;
        "#,
    )
    .unwrap();
    assert_eq!(ctx.eval("out").unwrap().as_smi(), Some(3));
}
