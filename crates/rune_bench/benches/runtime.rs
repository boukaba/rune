use criterion::{Criterion, criterion_group, criterion_main};
use rune_embed::Context;

fn bench_loop_sum_smi(c: &mut Criterion) {
    let src = "var s=0; for (var i=0; i<1000000; i=i+1) { s=s+i; } s";
    c.bench_function("loop_sum_smi_1M", |b| {
        b.iter(|| {
            let mut ctx = Context::new();
            ctx.eval(src).unwrap()
        })
    });
}

fn bench_array_push_grow(c: &mut Criterion) {
    let src = "var a=[]; for (var i=0; i<100000; i=i+1) { a.push(i); } a.length";
    c.bench_function("array_push_grow_100k", |b| {
        b.iter(|| {
            let mut ctx = Context::new();
            ctx.eval(src).unwrap()
        })
    });
}

fn bench_proto_chain_lookup(c: &mut Criterion) {
    let src = r#"
        function mk(level){ if(level==0){ return {x:42}; } var o={}; o.__proto__=mk(level-1); return o; }
        var o=mk(5); var s=0;
        for (var i=0; i<1000000; i=i+1) { s=s+o.x; }
        s
    "#;
    c.bench_function("proto_chain_lookup_5deep_1M", |b| {
        b.iter(|| {
            let mut ctx = Context::new();
            ctx.eval(src).unwrap()
        })
    });
}

criterion_group!(
    benches,
    bench_loop_sum_smi,
    bench_array_push_grow,
    bench_proto_chain_lookup
);
criterion_main!(benches);
