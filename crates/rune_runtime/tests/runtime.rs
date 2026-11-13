use rune_interpreter::builtins::value_to_js_string;
use rune_runtime::Runtime;

fn eval_str(rt: &mut Runtime, source: &str) -> String {
    value_to_js_string(rt.context_mut().eval(source).unwrap())
}

#[test]
fn timers_run_after_eval_in_order() {
    let mut rt = Runtime::new();
    rt.eval(
        r#"
        var out = "";
        setTimeout(function () { out += "a"; }, 0);
        setTimeout(function () { out += "b"; }, 0);
        "#,
    )
    .unwrap();
    assert_eq!(eval_str(&mut rt, "out"), "ab");
}

#[test]
fn timer_args_clear_and_nested_timers() {
    let mut rt = Runtime::new();
    rt.eval(
        r#"
        var got = "";
        var id = setTimeout(function (a, b) { got = a + b; }, 0, "x", "y");
        var bad = setTimeout(function () { got = "bad"; }, 0);
        clearTimeout(bad);
        setTimeout(function () {
            setTimeout(function () { got += "!"; }, 0);
        }, 0);
        "#,
    )
    .unwrap();
    assert_eq!(eval_str(&mut rt, "got"), "xy!");
}

#[test]
fn timer_promise_microtasks_drain() {
    let mut rt = Runtime::new();
    rt.eval(
        r#"
        var out = "";
        setTimeout(function () {
            Promise.resolve(1).then(function (v) { out = "p" + v; });
        }, 0);
        "#,
    )
    .unwrap();
    assert_eq!(eval_str(&mut rt, "out"), "p1");
}

#[test]
fn host_functions_compose_with_runtime() {
    let mut rt = Runtime::new();
    rt.context_mut()
        .register_host_fn("hostAdd", 2, |_gc, _this, args, _vm| {
            let a = args[0].as_smi().unwrap_or(0);
            let b = args[1].as_smi().unwrap_or(0);
            rune_core::value::Value::smi(a + b)
        });
    let v = rt.eval("hostAdd(20, 22)").unwrap();
    assert_eq!(v.as_smi(), Some(42));
}
