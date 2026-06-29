use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use rune_core::value::Value;
use rune_embed::Context;

/// Helper: extract the numeric value from a Value regardless of encoding.
fn to_i64(val: Value) -> i64 {
    val.as_smi()
        .map(|v| v as i64)
        .or_else(|| val.as_float64().map(|v| v as i64))
        .unwrap_or_else(|| panic!("benchmark produced non-numeric result: {val:?}"))
}

fn bench_loop_sum_smi(c: &mut Criterion) {
    let src = "var s=0; for (var i=0; i<1000000; i=i+1) { s=s+i; } s";
    c.bench_function("loop_sum_smi_1M", |b| {
        b.iter_batched(
            Context::new,
            |mut ctx| {
                let val = ctx.eval(src).unwrap();
                assert_eq!(to_i64(val), 499_999_500_000);
                val
            },
            BatchSize::SmallInput,
        )
    });
}

fn bench_array_push_grow(c: &mut Criterion) {
    let src = "var a=[]; for (var i=0; i<100000; i=i+1) { a.push(i); } a.length";
    c.bench_function("array_push_grow_100k", |b| {
        b.iter_batched(
            Context::new,
            |mut ctx| {
                let val = ctx.eval(src).unwrap();
                assert_eq!(to_i64(val), 100_000);
                val
            },
            BatchSize::SmallInput,
        )
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
        b.iter_batched(
            Context::new,
            |mut ctx| {
                let val = ctx.eval(src).unwrap();
                assert_eq!(to_i64(val), 42_000_000);
                val
            },
            BatchSize::SmallInput,
        )
    });
}

/// JIT steady-state: add() called 1M times within a single eval triggers tier-up at 50.
fn bench_jit_hot_function(c: &mut Criterion) {
    let src =
        "function add(a,b){ return a+b; } var s=0; for(var i=0;i<1000000;i=i+1){ s=add(s,i); } s";
    c.bench_function("jit_hot_function_1M", |b| {
        b.iter_batched(
            Context::new,
            |mut ctx| {
                let val = ctx.eval(src).unwrap();
                assert_eq!(to_i64(val), 499_999_500_000);
                val
            },
            BatchSize::SmallInput,
        )
    });
}

/// JIT with inlining: same benchmark as above but with enable_inlining=true.
fn bench_jit_hot_function_inline(c: &mut Criterion) {
    let src =
        "function add(a,b){ return a+b; } var s=0; for(var i=0;i<1000000;i=i+1){ s=add(s,i); } s";
    c.bench_function("jit_hot_function_inline_1M", |b| {
        b.iter_batched(
            || {
                let mut ctx = Context::new();
                ctx.enable_inlining = true;
                ctx
            },
            |mut ctx| {
                let val = ctx.eval(src).unwrap();
                assert_eq!(to_i64(val), 499_999_500_000);
                val
            },
            BatchSize::SmallInput,
        )
    });
}

/// Polymorphic property access with 10 shapes at one callsite — SIDT stays O(1).
fn bench_polymorphic_property_access(c: &mut Criterion) {
    // 10 shapes cycled via 1000 element array, 1M total accesses (single loop)
    let src = r#"
        var objs = [];
        var i = 0;
        while (i < 10) {
            var o = {};
            o["k" + i] = i;
            o.x = i;
            objs.push(o);
            i = i + 1;
        }
        var j = 10;
        while (j < 1000) {
            objs.push(objs[j - 10]);
            j = j + 1;
        }
        var s = 0;
        var i = 0;
        while (i < 1000000) {
            s = s + objs[i % 1000].x;
            i = i + 1;
        }
        s
    "#;
    c.bench_function("poly_prop_10shapes_1M", |b| {
        b.iter_batched(
            Context::new,
            |mut ctx| {
                let val = ctx.eval(src).unwrap();
                assert_eq!(to_i64(val), 4_500_000);
                val
            },
            BatchSize::SmallInput,
        )
    });
}

/// Parse + emit + execute time (Context already created — semispace alloc not included).
/// Actual cold-start with Context::new() is impractical to benchmark here due to 8 MB
/// semispace allocation × thousands of criterion iterations causing OOM on macOS.
fn bench_parse_emit_execute(c: &mut Criterion) {
    let mut ctx = Context::new();
    c.bench_function("parse_emit_execute_hello", |b| {
        b.iter(|| ctx.eval("'hello'").unwrap())
    });
}

const JSON_ROUND_TRIP_SRC: &str = include_str!("../scripts/json_round_trip.js");

/// Full cold-start: create new Context + parse + emit + execute.
fn bench_json_round_trip(c: &mut Criterion) {
    c.bench_function("json_round_trip_1k", |b| {
        b.iter_batched(
            Context::new,
            |mut ctx| {
                let val = ctx.eval(JSON_ROUND_TRIP_SRC).unwrap();
                assert_eq!(to_i64(val), 166_833);
                val
            },
            BatchSize::SmallInput,
        )
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default();
    targets = bench_loop_sum_smi, bench_array_push_grow, bench_proto_chain_lookup,
        bench_jit_hot_function, bench_jit_hot_function_inline, bench_polymorphic_property_access,
        bench_parse_emit_execute, bench_json_round_trip,
}
criterion_main!(benches);
