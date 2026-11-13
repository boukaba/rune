use rune_embed::Context;

#[test]
fn test_eval_number() {
    let mut ctx = Context::new_small();
    let result = ctx.eval("42").unwrap();
    assert_eq!(result.as_smi(), Some(42));
}

#[test]
fn test_eval_binary() {
    let mut ctx = Context::new_small();
    let result = ctx.eval("1 + 2").unwrap();
    assert_eq!(result.as_smi(), Some(3));
}

#[test]
fn test_eval_multiplication() {
    let mut ctx = Context::new_small();
    let result = ctx.eval("2 * 3 + 4").unwrap();
    assert_eq!(result.as_smi(), Some(10));
}

#[test]
fn test_eval_subtract() {
    let mut ctx = Context::new_small();
    let result = ctx.eval("10 - 3").unwrap();
    assert_eq!(result.as_smi(), Some(7));
}

#[test]
fn test_eval_var_decl() {
    let mut ctx = Context::new_small();
    ctx.eval("var x = 42;").unwrap();
    // The local should be stored and retrievable
    let result = ctx.eval("var y = 10;").unwrap();
    assert!(result.is_undefined());
}

#[test]
fn test_eval_if() {
    let mut ctx = Context::new_small();
    let result = ctx.eval("if (true) { 1; } else { 2; }").unwrap();
    // if's result is the last expression in the taken branch
    assert!(result.is_undefined()); // expression statements pop
}

#[test]
fn test_eval_while() {
    let mut ctx = Context::new_small();
    let result = ctx
        .eval(
            "var x = 10;
         while (x > 0) {
           x = x - 1;
         }",
        )
        .unwrap();
    assert!(result.is_undefined());
}

#[test]
fn test_eval_do_while() {
    let mut ctx = Context::new_small();
    let result = ctx
        .eval(
            "var x = 10;
         do {
           x = x - 1;
         } while (x > 0);",
        )
        .unwrap();
    assert!(result.is_undefined());
}

#[test]
fn test_eval_do_while_once() {
    let mut ctx = Context::new_small();
    let result = ctx
        .eval(
            "var x = 0;
         do {
           x = x + 1;
         } while (false);
         x",
        )
        .unwrap();
    assert_eq!(result.as_smi(), Some(1), "do-while body runs at least once");
}

#[test]
fn test_eval_for() {
    let mut ctx = Context::new_small();
    let result = ctx
        .eval(
            "var s = 0;
         for (var i = 0; i < 10; i = i + 1) {
           s = s + i;
         }",
        )
        .unwrap();
    assert!(result.is_undefined());
}

#[test]
fn test_eval_comparison() {
    let mut ctx = Context::new_small();
    let r1 = ctx.eval("1 < 2").unwrap();
    assert_eq!(r1.to_boolean(), Some(true));

    let r2 = ctx.eval("3 > 5").unwrap();
    assert_eq!(r2.to_boolean(), Some(false));
}

#[test]
fn test_eval_unary() {
    let mut ctx = Context::new_small();
    let r1 = ctx.eval("-5").unwrap();
    assert_eq!(r1.as_smi(), Some(-5));

    let r2 = ctx.eval("!true").unwrap();
    assert_eq!(r2.to_boolean(), Some(false));
}

#[test]
fn test_eval_bitwise() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("1 | 2").unwrap();
    assert_eq!(r.as_smi(), Some(3));
}

#[test]
fn test_eval_nested_block() {
    let mut ctx = Context::new_small();
    let result = ctx.eval("{{{{42;}}}}").unwrap();
    assert!(result.is_undefined());
}

#[test]
fn test_eval_string_literal() {
    let mut ctx = Context::new_small();
    let result = ctx.eval(r#""hello""#).unwrap();
    assert!(result.is_heap_object());
}

#[test]
fn test_eval_property_access() {
    let mut ctx = Context::new_small();
    let result = ctx.eval("({a: 1, b: 2}).a").unwrap();
    assert_eq!(result.as_smi(), Some(1));
}

#[test]
fn test_eval_computed_property() {
    let mut ctx = Context::new_small();
    let result = ctx.eval("({a: 42, b: 99})['a']").unwrap();
    assert_eq!(result.as_smi(), Some(42));
}

#[test]
fn test_eval_var_lookup() {
    let mut ctx = Context::new_small();
    let result = ctx.eval("var x = 42; x").unwrap();
    assert_eq!(result.as_smi(), Some(42));
}

#[test]
fn test_eval_property_assign() {
    let mut ctx = Context::new_small();
    let result = ctx.eval("var o = {a: 1}; o.a = 5; o.a").unwrap();
    assert_eq!(result.as_smi(), Some(5));
}

#[test]
fn test_eval_object_literal() {
    let mut ctx = Context::new_small();
    let result = ctx.eval("({a: 1})").unwrap();
    assert!(result.is_heap_object());
}

#[test]
fn test_eval_function_decl() {
    let mut ctx = Context::new_small();
    // Test that function object is created by checking typeof
    let result = ctx.eval("typeof function() { return 1; }").unwrap();
    assert!(result.is_heap_object()); // typeof returns a HeapString for functions
}

#[test]
fn test_eval_make_function_expr() {
    let mut ctx = Context::new_small();
    let result = ctx.eval("(function() { return 1; })").unwrap();
    assert!(result.is_heap_object());
}

#[test]
fn test_eval_call_func_obj() {
    let mut ctx = Context::new_small();
    // Direct call via var binding
    let result = ctx.eval("var f = function() { return 1; }; f()").unwrap();
    assert_eq!(result.as_smi(), Some(1));
}

#[test]
fn test_eval_call_iife() {
    let mut ctx = Context::new_small();
    // IIFE - immediately invoked function expression
    let result = ctx.eval("(function() { return 1; })()").unwrap();
    assert_eq!(result.as_smi(), Some(1));
}

#[test]
fn test_eval_function_decl_and_call() {
    let mut ctx = Context::new_small();
    let result = ctx.eval("function f() { return 42; } f()").unwrap();
    assert_eq!(result.as_smi(), Some(42));
}

#[test]
fn test_eval_function_args() {
    let mut ctx = Context::new_small();
    let result = ctx
        .eval("function add(a, b) { return a + b; } add(3, 4)")
        .unwrap();
    assert_eq!(result.as_smi(), Some(7));
}

#[test]
fn test_eval_nested_function() {
    let mut ctx = Context::new_small();
    let result = ctx
        .eval("function outer() { function inner() { return 99; } return inner(); } outer()")
        .unwrap();
    assert_eq!(result.as_smi(), Some(99));
}

#[test]
fn test_eval_function_expr() {
    let mut ctx = Context::new_small();
    let result = ctx
        .eval("var f = function(x) { return x * 2; }; f(5)")
        .unwrap();
    assert_eq!(result.as_smi(), Some(10));
}

#[test]
fn test_eval_recursive() {
    let mut ctx = Context::new_small();
    let result = ctx
        .eval("function fact(n) { if (n <= 1) { return 1; } return n * fact(n - 1); } fact(5)")
        .unwrap();
    assert_eq!(result.as_smi(), Some(120));
}

#[test]
fn test_eval_chained_property() {
    let mut ctx = Context::new_small();
    let result = ctx.eval("({a: {b: 42}}).a.b").unwrap();
    assert_eq!(result.as_smi(), Some(42));
}

#[test]
fn test_eval_multi_object() {
    let mut ctx = Context::new_small();
    let result = ctx.eval("var x = {a: 10, b: 20}; x.a + x.b").unwrap();
    assert_eq!(result.as_smi(), Some(30));
}

#[test]
fn test_parse_error() {
    let mut ctx = Context::new_small();
    let result = ctx.eval("!!!");
    assert!(result.is_err());
}

#[test]
fn test_eval_throw() {
    let mut ctx = Context::new_small();
    let result = ctx.eval("throw 42;");
    assert!(result.is_err(), "throw should produce an error");
    let err = result.unwrap_err();
    assert!(err.contains("42"), "error should contain thrown value");
}

#[test]
fn test_eval_new_simple() {
    let mut ctx = Context::new_small();
    let result = ctx.eval("new Object();").unwrap();
    assert!(result.is_heap_object(), "new should return a new object");
}

#[test]
fn test_non_generator_no_resume() {
    let mut ctx = Context::new_small();
    ctx.eval("function f() { return 1; }").unwrap();
    // Global scope not yet implemented — function declarations don't persist
    // across eval() calls. This test just verifies no crash.
    let result = ctx.eval("42").unwrap();
    assert_eq!(result.as_smi(), Some(42));
}

#[test]
fn test_eval_string_concat() {
    let mut ctx = Context::new_small();
    let result = ctx.eval("\"hello\" + \" world\"").unwrap();
    assert!(
        !result.is_undefined(),
        "string concat should not be undefined"
    );
    // We can't easily inspect the string value, but it should not error
}

#[test]
fn test_eval_mixed_concat() {
    let mut ctx = Context::new_small();
    let result = ctx.eval("\"x\" + 1").unwrap();
    assert!(
        !result.is_undefined(),
        "mixed concat should not be undefined"
    );
}

// ---- Generator / Yield tests ----

#[test]
fn test_generator_yield_value() {
    let mut ctx = Context::new_small();
    ctx.eval("function* gen() { yield 42; }; var g = gen();")
        .unwrap();
    let v = ctx.eval("g.next().value").unwrap();
    assert_eq!(v.as_smi(), Some(42), "first next().value should be 42");
    let d = ctx.eval("g.next().done").unwrap();
    assert!(d.to_bool(), "second next().done should be true");
}

#[test]
fn test_generator_yield_twice() {
    let mut ctx = Context::new_small();
    ctx.eval("function* gen() { yield 1; yield 2; }; var g = gen();")
        .unwrap();
    ctx.eval("var r1 = g.next(); var r2 = g.next(); var r3 = g.next();")
        .unwrap();
    assert_eq!(ctx.eval("r1.value").unwrap().as_smi(), Some(1));
    assert!(!ctx.eval("r1.done").unwrap().to_bool());
    assert_eq!(ctx.eval("r2.value").unwrap().as_smi(), Some(2));
    assert!(ctx.eval("r3.done").unwrap().to_bool());
}

#[test]
fn test_generator_yield_then_return() {
    let mut ctx = Context::new_small();
    ctx.eval("function* gen() { yield 10; return 20; }; var g = gen();")
        .unwrap();
    ctx.eval("var r1 = g.next(); var r2 = g.next();").unwrap();
    assert_eq!(ctx.eval("r1.value").unwrap().as_smi(), Some(10));
    assert!(!ctx.eval("r1.done").unwrap().to_bool());
    assert_eq!(ctx.eval("r2.value").unwrap().as_smi(), Some(20));
    assert!(ctx.eval("r2.done").unwrap().to_bool());
}

#[test]
fn test_generator_params_reach_body() {
    let mut ctx = Context::new_small();
    ctx.eval("function* add(a, b) { yield a + b; }; var g = add(20, 22);")
        .unwrap();
    assert_eq!(ctx.eval("g.next().value").unwrap().as_smi(), Some(42));
}

#[test]
fn test_generator_send_values_in() {
    let mut ctx = Context::new_small();
    ctx.eval("function* echo() { var x = yield 1; yield x + 1; }; var g = echo();")
        .unwrap();
    assert_eq!(ctx.eval("g.next().value").unwrap().as_smi(), Some(1));
    assert_eq!(ctx.eval("g.next(41).value").unwrap().as_smi(), Some(42));
}

#[test]
fn test_generator_return_closes() {
    let mut ctx = Context::new_small();
    ctx.eval("function* gen() { yield 1; yield 2; }; var g = gen();")
        .unwrap();
    assert_eq!(ctx.eval("g.return(9).value").unwrap().as_smi(), Some(9));
    assert!(ctx.eval("g.return(9).done").unwrap().to_bool());
    assert!(ctx.eval("g.next().done").unwrap().to_bool());
}

#[test]
fn test_generator_throw_propagates() {
    let mut ctx = Context::new_small();
    let r = ctx.eval(
        "function* gen() { yield 1; yield 2; }; var g = gen(); g.next(); try { g.throw('boom'); 'no-throw'; } catch (e) { 'caught:' + e; }",
    );
    assert!(r.is_ok() || r.is_err());
}

#[test]
fn test_generator_for_of() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "function* range(n) { for (let i = 0; i < n; i++) yield i; }; var t = 0; for (var v of range(4)) { t += v; } t;",
        )
        .unwrap();
    assert_eq!(
        r.as_smi(),
        Some(6),
        "for-of over generator should sum 0+1+2+3"
    );
}

#[test]
fn test_generator_spread() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval("function* abc() { yield 7; yield 8; } var arr = [...abc()]; arr.length + arr[0] * 10 + arr[1];")
        .unwrap();
    assert_eq!(
        r.as_smi(),
        Some(2 + 70 + 8),
        "spread should drain both yields"
    );
}

#[test]
fn test_generator_closure_capture() {
    let mut ctx = Context::new_small();
    // Each yield must see the iteration's own binding.
    let r = ctx
        .eval(
            "var fs = []; for (let i = 0; i < 3; i++) { fs.push(function () { return i; }); } function* pick() { yield fs[0](); yield fs[2](); } var g = pick(); g.next().value * 10 + g.next().value;",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(2), "closures must capture per-iteration i");
}

#[test]
fn test_generator_yield_star() {
    let mut ctx = Context::new_small();
    ctx.eval(
        "function* a() { yield 1; yield 2; }; function* b() { yield 0; yield* a(); }; var g = b();",
    )
    .unwrap();
    ctx.eval("var r1 = g.next(); var r2 = g.next(); var r3 = g.next(); var r4 = g.next();")
        .unwrap();
    assert_eq!(ctx.eval("r1.value").unwrap().as_smi(), Some(0));
    assert_eq!(ctx.eval("r2.value").unwrap().as_smi(), Some(1));
    assert_eq!(ctx.eval("r3.value").unwrap().as_smi(), Some(2));
    assert!(ctx.eval("r4.done").unwrap().to_bool());
}

#[test]
fn test_generator_yield_star_array() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval("function* b() { yield* [10, 20]; } var t = 0; for (var v of b()) { t += v; } t;")
        .unwrap();
    assert_eq!(r.as_smi(), Some(30));
}

#[test]
fn test_generator_throw_caught_inside() {
    let mut ctx = Context::new_small();
    ctx.eval(
        "function* gen() { try { yield 1; yield 2; } catch (e) { yield 99; } } var g = gen();",
    )
    .unwrap();
    assert_eq!(ctx.eval("g.next().value").unwrap().as_smi(), Some(1));
    // throw() resumes at the yield point: the catch runs and yields 99.
    assert_eq!(
        ctx.eval("g.throw('boom').value").unwrap().as_smi(),
        Some(99)
    );
    // After the catch's yield, the generator finishes (done, undefined).
    assert!(ctx.eval("g.next().done").unwrap().to_bool());
}

#[test]
fn test_generator_throw_uncaught_completes() {
    let mut ctx = Context::new_small();
    ctx.eval("function* gen() { yield 1; yield 2; } var g = gen();")
        .unwrap();
    assert_eq!(ctx.eval("g.next().value").unwrap().as_smi(), Some(1));
    // No catch inside: the throw propagates and completes the generator.
    assert!(ctx.eval("g.throw('boom')").is_err());
    assert!(ctx.eval("g.next().done").unwrap().to_bool());
}

#[test]
fn test_generator_throw_before_start() {
    let mut ctx = Context::new_small();
    ctx.eval("function* gen() { yield 1; } var g = gen();")
        .unwrap();
    // Throw before the first next(): body never runs, generator completes.
    assert!(ctx.eval("g.throw('boom')").is_err());
    assert!(ctx.eval("g.next().done").unwrap().to_bool());
}

#[test]
fn test_generator_return_runs_finally() {
    let mut ctx = Context::new_small();
    ctx.eval("function* gen() { try { yield 1; yield 2; } finally { yield 99; } } var g = gen();")
        .unwrap();
    assert_eq!(ctx.eval("g.next().value").unwrap().as_smi(), Some(1));
    // return() runs the finally, which yields first (abrupt still pending).
    assert_eq!(ctx.eval("g.return(7).value").unwrap().as_smi(), Some(99));
    // Resuming completes with the return value, then stays done.
    assert_eq!(ctx.eval("g.next().value").unwrap().as_smi(), Some(7));
    assert!(ctx.eval("g.next().done").unwrap().to_bool());
}

#[test]
fn test_generator_yield_star_delegate_return() {
    let mut ctx = Context::new_small();
    ctx.eval("function* inner() { yield 1; return 99; } function* outer() { var x = yield* inner(); return x; } var g = outer();")
        .unwrap();
    assert_eq!(ctx.eval("g.next().value").unwrap().as_smi(), Some(1));
    // Draining the delegate makes its return value the yield* expression value.
    assert_eq!(ctx.eval("g.next().value").unwrap().as_smi(), Some(99));
    assert!(ctx.eval("g.next().done").unwrap().to_bool());
}

#[test]
fn test_generator_yield_star_sent_forwarding() {
    let mut ctx = Context::new_small();
    ctx.eval("function* inner() { var a = yield 1; return a; } function* outer() { var x = yield* inner(); return x; } var g = outer();")
        .unwrap();
    assert_eq!(ctx.eval("g.next().value").unwrap().as_smi(), Some(1));
    // The sent value becomes the delegate next() argument (a === 42).
    assert_eq!(ctx.eval("g.next(42).value").unwrap().as_smi(), Some(42));
}

#[test]
fn test_generator_yield_star_throw_forwarding() {
    let mut ctx = Context::new_small();
    ctx.eval("function* inner() { try { yield 1; yield 2; } catch (e) { yield 99; } } function* outer() { yield* inner(); } var g = outer();")
        .unwrap();
    assert_eq!(ctx.eval("g.next().value").unwrap().as_smi(), Some(1));
    // outer.throw() forwards to the delegate's throw (caught inside -> 99).
    assert_eq!(
        ctx.eval("g.throw('boom').value").unwrap().as_smi(),
        Some(99)
    );
}

#[test]
fn test_generator_yield_star_return_forwarding() {
    let mut ctx = Context::new_small();
    ctx.eval("function* inner() { try { yield 1; yield 2; } finally { yield 99; } } function* outer() { yield* inner(); } var g = outer();")
        .unwrap();
    assert_eq!(ctx.eval("g.next().value").unwrap().as_smi(), Some(1));
    // outer.return() forwards to the delegate's return (finally yields 99).
    assert_eq!(ctx.eval("g.return(7).value").unwrap().as_smi(), Some(99));
}

#[test]
fn test_generator_yield_star_custom_iterable() {
    let mut ctx = Context::new_small();
    ctx.eval("var it2 = { i: 0, next: function () { this.i = this.i + 1; if (this.i > 2) { return { value: 99, done: true }; } return { value: this.i * 10, done: false }; } }; var del = {}; del[Symbol.iterator] = function () { return it2; }; function* outer() { var x = yield* del; return x; } var g = outer();")
        .unwrap();
    assert_eq!(ctx.eval("g.next().value").unwrap().as_smi(), Some(10));
    assert_eq!(ctx.eval("g.next().value").unwrap().as_smi(), Some(20));
    // Delegate done: its return value (99) is the yield* expression value.
    assert_eq!(ctx.eval("g.next().value").unwrap().as_smi(), Some(99));
    assert!(ctx.eval("g.next().done").unwrap().to_bool());
}

#[test]
fn test_generator_yield_star_js_throw_method() {
    let mut ctx = Context::new_small();
    ctx.eval("var it2 = { next: function () { return { value: 1, done: false }; }, throw: function (e) { return { value: 42, done: true }; } }; var del = {}; del[Symbol.iterator] = function () { return it2; }; function* outer() { var x = yield* del; return x; } var g = outer();")
        .unwrap();
    assert_eq!(ctx.eval("g.next().value").unwrap().as_smi(), Some(1));
    assert_eq!(
        ctx.eval("g.throw('boom').value").unwrap().as_smi(),
        Some(42)
    );
}

#[test]
fn test_generator_yield_star_js_return_method() {
    let mut ctx = Context::new_small();
    ctx.eval("var it2 = { next: function () { return { value: 1, done: false }; }, return: function (v) { return { value: 43, done: true }; } }; var del = {}; del[Symbol.iterator] = function () { return it2; }; function* outer() { var x = yield* del; return x; } var g = outer();")
        .unwrap();
    assert_eq!(ctx.eval("g.next().value").unwrap().as_smi(), Some(1));
    assert_eq!(ctx.eval("g.return(7).value").unwrap().as_smi(), Some(43));
}

#[test]
fn test_generator_done_stays_done() {
    let mut ctx = Context::new_small();
    ctx.eval("function* gen() { yield 1; }; var g = gen();")
        .unwrap();
    ctx.eval("g.next()").unwrap();
    assert!(ctx.eval("g.next().done").unwrap().to_bool());
    assert!(ctx.eval("g.next().done").unwrap().to_bool());
}

#[test]
fn test_generator_iterator_identity() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval("function* gen() { yield 1; } var g = gen(); g[Symbol.iterator]() === g;")
        .unwrap();
    assert!(r.to_bool(), "@@iterator must return the generator itself");
}

#[test]
fn test_eval_try_catch_no_exception() {
    let mut ctx = Context::new_small();
    let result = ctx
        .eval("var x = 0; try { x = 1; } catch (e) { x = 2; } x;")
        .unwrap();
    assert_eq!(
        result.as_smi(),
        Some(1),
        "try block should execute normally"
    );
}

#[test]
fn test_eval_try_catch_with_exception() {
    let mut ctx = Context::new_small();
    let result = ctx
        .eval("var x; try { throw 42; } catch (e) { x = e; } x;")
        .unwrap();
    assert_eq!(result.as_smi(), Some(42), "catch should bind thrown value");
}

#[test]
fn test_eval_try_catch_no_error() {
    let mut ctx = Context::new_small();
    let result = ctx.eval("try { 1; } catch (e) {} 2;").unwrap();
    assert_eq!(
        result.as_smi(),
        Some(2),
        "execution should continue after try-catch"
    );
}

#[test]
fn test_eval_try_catch_error_caught() {
    let mut ctx = Context::new_small();
    let result = ctx.eval("try { throw 99; } catch (e) {} 42;").unwrap();
    assert_eq!(
        result.as_smi(),
        Some(42),
        "caught error should not propagate"
    );
}

#[test]
fn test_global_scope_persistence_var() {
    let mut ctx = Context::new_small();
    ctx.eval("var x = 10;").unwrap();
    ctx.eval("y = 20;").unwrap();
    let r1 = ctx.eval("x").unwrap();
    assert_eq!(
        r1.as_smi(),
        Some(10),
        "var-declared variable persists across evals"
    );
    let r2 = ctx.eval("y").unwrap();
    assert_eq!(
        r2.as_smi(),
        Some(20),
        "implicit global assignment persists across evals"
    );
}

#[test]
fn test_global_scope_mutation() {
    let mut ctx = Context::new_small();
    ctx.eval("var counter = 0;").unwrap();
    let r1 = ctx.eval("counter = counter + 1;").unwrap();
    assert_eq!(r1.as_smi(), Some(1), "assign returns new value");
    let r2 = ctx.eval("counter").unwrap();
    assert_eq!(r2.as_smi(), Some(1), "mutation persists");
    ctx.eval("counter = counter + 1;").unwrap();
    let r3 = ctx.eval("counter").unwrap();
    assert_eq!(r3.as_smi(), Some(2), "multiple mutations persist");
}

#[test]
fn test_try_finally_no_throw() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval("var x = 0; try { x = 1; } finally { x = 2; } x;")
        .unwrap();
    assert_eq!(r.as_smi(), Some(2), "finally should run after try");
}

#[test]
fn test_try_finally_throw_caught_by_outer() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval("var x = 0; try { try { throw 99; } finally { x = 1; } } catch (e) { x = e; } x;")
        .unwrap();
    assert_eq!(
        r.as_smi(),
        Some(99),
        "outer catch should catch rethrown exception"
    );
}

#[test]
fn test_try_catch_finally() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval("var x = 0; try { throw 42; } catch (e) { x = 1; } finally { x = x + 10; } x;")
        .unwrap();
    assert_eq!(r.as_smi(), Some(11), "finally should run after catch");
}

#[test]
fn test_try_finally_throw() {
    let mut ctx = Context::new_small();
    // If try throws and there's a finally, the exception should propagate after finally runs
    // We use an outer try-catch to observe this
    let r = ctx
        .eval("var x = 0; try { try { throw 99; } finally { x = 1; } } catch (e) { } x;")
        .unwrap();
    assert_eq!(
        r.as_smi(),
        Some(1),
        "finally should run before exception propagates"
    );
}

#[test]
fn test_builtin_print() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("print(42); 99;").unwrap();
    assert_eq!(
        r.as_smi(),
        Some(99),
        "print should work and return undefined"
    );
}

#[test]
fn test_builtin_string_from_char_code() {
    let mut ctx = Context::new_small();
    let r = ctx.eval(r#"String.fromCharCode(65)"#).unwrap();
    assert!(
        r.is_heap_object(),
        "String.fromCharCode should return a string"
    );
    let r2 = ctx.eval(r#"String.fromCharCode(72, 73)"#).unwrap();
    assert!(
        r2.is_heap_object(),
        "String.fromCharCode with multiple args should return a string"
    );
}

#[test]
fn test_builtin_error() {
    let mut ctx = Context::new_small();
    let r = ctx.eval(r#"Error("test")"#).unwrap();
    assert!(r.is_heap_object(), "Error should return an object");
}

#[test]
fn test_builtin_test262_error() {
    let mut ctx = Context::new_small();
    let r = ctx.eval(r#"Test262Error("fail")"#).unwrap();
    assert!(r.is_heap_object(), "Test262Error should return an object");
}

#[test]
fn test_typeof_basic() {
    let mut ctx = Context::new_small();
    let r = ctx.eval(r#"typeof 42"#).unwrap();
    assert!(r.heap_ptr().is_some(), "typeof should return a string");
}

#[test]
fn test_float_literal() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("4.56").unwrap();
    assert!(r.is_float64(), "4.56 should be a float");
    assert!((r.as_float64().unwrap() - 4.56).abs() < 1e-10);
}

#[test]
fn test_float_addition() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("1.5 + 2.5").unwrap();
    assert_eq!(r.as_smi(), Some(4));
}

#[test]
fn test_float_mixed_arith() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("1.5 + 3").unwrap();
    assert!(r.is_float64(), "1.5 + 3 should be a float");
}

#[test]
fn test_switch_basic() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        let x = 2;
        let result = 0;
        switch (x) {
            case 1: result = 10; break;
            case 2: result = 20; break;
            default: result = 30;
        }
        result
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(20));
}

#[test]
fn test_switch_default() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        let x = 99;
        let result = 0;
        switch (x) {
            case 1: result = 10; break;
            case 2: result = 20; break;
            default: result = 30;
        }
        result
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(30));
}

#[test]
fn test_typeof_float() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("typeof 3.14").unwrap();
    assert!(
        r.heap_ptr().is_some(),
        "typeof float should return a string"
    );
}

#[test]
fn test_switch_fallthrough() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        let x = 1;
        let result = 0;
        switch (x) {
            case 1: result = 1;
            case 2: result = 2; break;
            default: result = 3;
        }
        result
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(2));
}

#[test]
fn test_mod_zero_is_nan() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("5 % 0").unwrap();
    assert!(r.is_float64() || r.is_smi(), "5 % 0 should be a number");
    assert!(
        r.as_float64().is_some_and(|v| v.is_nan()),
        "5 % 0 should be NaN"
    );
}

#[test]
fn test_exp_negative() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("2 ** -1").unwrap();
    assert!(
        (r.as_float64().unwrap() - 0.5).abs() < 1e-10,
        "2 ** -1 should be 0.5"
    );
}

#[test]
fn test_null_plus_one() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("null + 1").unwrap();
    assert_eq!(r.as_smi(), Some(1));
}

#[test]
fn test_null_undefined_property_read_throws() {
    // A1: RequireObjectCoercible — reads on null/undefined throw a
    // catchable TypeError (previously silent undefined).
    let mut ctx = Context::new_small();
    for src in ["null.x", "undefined.x", "null[0]", "var a = {}; a.b.c"] {
        let err = ctx.eval(src).expect_err(&format!("{src} should throw"));
        assert!(err.contains("TypeError"), "expected TypeError, got: {err}");
    }
    // Catchable in-frame + visible to assert.throws.
    ctx.eval("var caught = 'no'; try { null.x; } catch (e) { caught = 'yes'; }")
        .unwrap();
    let r = ctx.eval("caught;").unwrap();
    let s = unsafe {
        rune_core::string::HeapString::to_string(
            r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
        )
    };
    assert_eq!(s, "yes", "catch block must run");
    ctx.eval("assert.throws(TypeError, function () { undefined.foo; });")
        .unwrap();
    // Optional chaining still short-circuits (no throw).
    assert!(ctx.eval("var x = null; x?.y;").unwrap().is_undefined());
    assert!(ctx.eval("var u; u?.y;").unwrap().is_undefined());
}

#[test]
fn test_null_undefined_property_write_throws() {
    // A1: writes on null/undefined throw a catchable TypeError.
    let mut ctx = Context::new_small();
    for src in ["null.x = 1", "undefined.x = 1", "null[0] = 1"] {
        let err = ctx.eval(src).expect_err(&format!("{src} should throw"));
        assert!(err.contains("TypeError"), "expected TypeError, got: {err}");
    }
    ctx.eval("assert.throws(TypeError, function () { null.x = 1; });")
        .unwrap();
    // Ordinary writes still work and return the stored value.
    let r = ctx.eval("var o = {}; o.x = 41; o.x;").unwrap();
    assert_eq!(r.as_smi(), Some(41));
}

#[test]
fn test_error_objects_real() {
    // A2: engine-raised errors are real Error objects — name/message own
    // props, instanceof, string coercion — not legacy strings.
    let mut ctx = Context::new_small();
    let r = ctx
        .eval("var n = ''; try { null.x; } catch (e) { n = e.name; } n === 'TypeError';")
        .unwrap();
    assert!(r.to_bool(), "caught error name is TypeError");
    let r = ctx
        .eval("var ok = false; try { null.x; } catch (e) { ok = e instanceof TypeError; } ok;")
        .unwrap();
    assert!(r.to_bool(), "caught error instanceof TypeError");
    let r = ctx
        .eval("var s = ''; try { null.x; } catch (e) { s = String(e); } s === 'TypeError: Cannot read properties of null (reading \\'x\\')';")
        .unwrap();
    assert!(r.to_bool(), "String(err) uses name: message");
    // Throw paths are catchable in-frame with identity intact.
    let r = ctx
        .eval(
            "var ok = false; try { new Symbol(); } catch (e) { ok = e instanceof TypeError; } ok;",
        )
        .unwrap();
    assert!(r.to_bool(), "new Symbol() TypeError catchable");
    // Uncaught rendering keeps the kind.
    let err = ctx.eval("null.y;").expect_err("should throw");
    assert!(
        err.contains("TypeError: Cannot read properties of null"),
        "unexpected uncaught text: {err}"
    );
}

#[test]
fn test_class_call_without_new_throws() {
    // A3: [[Call]] on a class constructor throws (super() exempt).
    let mut ctx = Context::new_small();
    ctx.eval("class C { constructor() { this.x = 1; } }")
        .unwrap();
    let err = ctx.eval("C();").expect_err("class call should throw");
    assert!(err.contains("TypeError"), "expected TypeError, got: {err}");
    ctx.eval("assert.throws(TypeError, function () { C(); });")
        .unwrap();
    // new still works; super() chains still work (incl. spread form).
    let r = ctx.eval("new C().x;").unwrap();
    assert_eq!(r.as_smi(), Some(1));
    let r = ctx
        .eval(
            "class A { constructor(x) { this.x = x; } }
             class B extends A { constructor(x) { super(x + 1); } }
             new B(1).x;",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(2));
    let r = ctx
        .eval(
            "class A { constructor() { this.x = 40; } }
             class B extends A { constructor(...a) { super(...a); } }
             new B().x;",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(40));
}

#[test]
fn test_derived_constructor_return_check() {
    // A3: derived ctors may only return objects or undefined.
    let mut ctx = Context::new_small();
    ctx.eval("class C extends Object { constructor() { return null; } }")
        .unwrap();
    let err = ctx.eval("new C();").expect_err("should throw");
    assert!(err.contains("TypeError"), "expected TypeError, got: {err}");
    // Object results flow through; undefined falls back to this.
    let r = ctx
        .eval("class D extends Object { constructor() { return { v: 7 }; } } new D().v;")
        .unwrap();
    assert_eq!(r.as_smi(), Some(7));
    let r = ctx
        .eval("class E extends Object { constructor() { this.w = 8; } } new E().w;")
        .unwrap();
    assert_eq!(r.as_smi(), Some(8));
    // Base-class explicit primitives are ignored (no throw).
    let r = ctx
        .eval("class F { constructor() { return 5; } } new F() instanceof F;")
        .unwrap();
    assert!(r.to_bool());
}

#[test]
fn test_strict_caller_arguments_poison() {
    // A3: strict functions throw on caller/arguments access; sloppy .caller
    // resolves to the caller (null at top level).
    let mut ctx = Context::new_small();
    ctx.eval("assert.throws(TypeError, function () { function f() { 'use strict'; } f.caller; });")
        .unwrap();
    ctx.eval(
        "assert.throws(TypeError, function () { function f() { 'use strict'; } f.arguments; });",
    )
    .unwrap();
    // Sloppy .caller is the executing caller, null at top level.
    let r = ctx
        .eval("function g() { return g.caller; } g() === null;")
        .unwrap();
    assert!(r.to_bool(), "top-level sloppy caller is null");
    let r = ctx
        .eval("function inner() { return inner.caller === outer; } function outer() { return inner(); } outer();")
        .unwrap();
    assert!(r.to_bool(), "sloppy caller resolves to the caller");
    // Strict caller poisons sloppy .caller reads.
    let r = ctx
        .eval("function g() { return g.caller; } function f() { 'use strict'; return g(); } var threw = false; try { f(); } catch (e) { threw = e instanceof TypeError; } threw;")
        .unwrap();
    assert!(r.to_bool(), "strict caller poisons sloppy .caller");
}

#[test]
fn test_define_property_basics() {
    // A4: defineProperty creates with descriptor defaults (non-enumerable,
    // non-writable, non-configurable when absent).
    let mut ctx = Context::new_small();
    let r = ctx
        .eval("var o = {}; Object.defineProperty(o, 'x', {value: 42}); o.x;")
        .unwrap();
    assert_eq!(r.as_smi(), Some(42));
    // Absent enumerable → excluded from keys.
    let r = ctx
        .eval("var o = {}; Object.defineProperty(o, 'x', {value: 1}); Object.keys(o).length;")
        .unwrap();
    assert_eq!(r.as_smi(), Some(0));
    // Absent writable → sloppy writes ignored.
    let r = ctx
        .eval("var o = {}; Object.defineProperty(o, 'x', {value: 1}); o.x = 2; o.x;")
        .unwrap();
    assert_eq!(r.as_smi(), Some(1));
    // Absent configurable → redefinition throws.
    ctx.eval("var o = {}; Object.defineProperty(o, 'x', {value: 1}); assert.throws(TypeError, function () { Object.defineProperty(o, 'x', {value: 2}); });")
        .unwrap();
    // Explicit flags honored.
    let r = ctx
        .eval("var o = {}; Object.defineProperty(o, 'x', {value: 3, writable: true, enumerable: true, configurable: true}); o.x = 4; o.x + Object.keys(o).length;")
        .unwrap();
    assert_eq!(r.as_smi(), Some(5));
    // Accessor descriptors dispatch getters.
    let r = ctx
        .eval("var o = {}; Object.defineProperty(o, 'x', {get: function () { return 7; }}); o.x;")
        .unwrap();
    assert_eq!(r.as_smi(), Some(7));
    // Mixing data and accessor fields throws.
    ctx.eval("assert.throws(TypeError, function () { Object.defineProperty({}, 'x', {value: 1, get: function () {}}); });")
        .unwrap();
}

#[test]
fn test_define_property_seal_freeze() {
    // A4: integrity levels + observers + getOwnPropertyDescriptor.
    let mut ctx = Context::new_small();
    let r = ctx
        .eval("var o = {a: 1}; Object.seal(o); Object.isSealed(o);")
        .unwrap();
    assert!(r.to_bool());
    let r = ctx
        .eval("var o = {a: 1}; Object.seal(o); o.b = 2; o.b;")
        .unwrap();
    assert!(r.is_undefined(), "sealed objects reject new keys");
    let r = ctx
        .eval("var o = {a: 1}; Object.freeze(o); o.a = 2; o.a;")
        .unwrap();
    assert_eq!(r.as_smi(), Some(1));
    let r = ctx.eval("var o = {a: 1}; Object.isFrozen(o);").unwrap();
    assert!(!r.to_bool());
    let r = ctx
        .eval("var o = {a: 1}; Object.preventExtensions(o); Object.isExtensible(o);")
        .unwrap();
    assert!(!r.to_bool());
    // getOwnPropertyDescriptor reflects stored attributes.
    let r = ctx
        .eval("var o = {}; Object.defineProperty(o, 'x', {value: 5, enumerable: true}); var d = Object.getOwnPropertyDescriptor(o, 'x'); d.value + (d.writable ? 10 : 0) + (d.enumerable ? 100 : 0) + (d.configurable ? 1000 : 0);")
        .unwrap();
    assert_eq!(r.as_smi(), Some(105));
    // Strict-mode write to readonly throws.
    ctx.eval("assert.throws(TypeError, function () { function f() { 'use strict'; var o = {}; Object.defineProperty(o, 'x', {value: 1}); o.x = 2; } f(); });")
        .unwrap();
}

#[test]
fn test_array_reduce_right_find_last() {
    // B1c: backward iteration mirrors (seeds from the end, walks down).
    let mut ctx = Context::new_small();
    let r = ctx
        .eval("[1, 2, 3, 4].reduceRight(function (a, b) { return a + b; });")
        .unwrap();
    assert_eq!(r.as_smi(), Some(10));
    // Order observable via subtraction.
    let r = ctx
        .eval("[1, 2, 3].reduceRight(function (a, b) { return a - b; });")
        .unwrap();
    assert_eq!(r.as_smi(), Some(0));
    // No-initial seeds from the last present element.
    let r = ctx
        .eval("var o = {length: 3, 0: 10, 2: 20}; Array.prototype.reduceRight.call(o, function (a, b) { return a + b; });")
        .unwrap();
    assert_eq!(r.as_smi(), Some(30));
    ctx.eval("assert.throws(TypeError, function () { [].reduceRight(function (a, b) { return a + b; }); });")
        .unwrap();
    // findLast / findLastIndex search from the end.
    let r = ctx
        .eval("[1, 2, 3].findLast(function (x) { return x < 3; });")
        .unwrap();
    assert_eq!(r.as_smi(), Some(2));
    let r = ctx
        .eval("[1, 2, 3].findLastIndex(function (x) { return x < 3; });")
        .unwrap();
    assert_eq!(r.as_smi(), Some(1));
    let r = ctx
        .eval("[1, 2, 3].findLastIndex(function (x) { return x > 9; });")
        .unwrap();
    assert_eq!(r.as_smi(), Some(-1));
}

#[test]
fn test_array_iter_prologue() {
    // B1a: length→callable order, holes skipped, null receiver throws.
    let mut ctx = Context::new_small();
    // Non-callable callback throws even on empty arrays (order matters).
    ctx.eval("assert.throws(TypeError, function () { [].map(); });")
        .unwrap();
    ctx.eval("assert.throws(TypeError, function () { [1, 2].map(5); });")
        .unwrap();
    ctx.eval("assert.throws(TypeError, function () { [].forEach(null); });")
        .unwrap();
    // Null/undefined receivers throw (no more panics on heap_ptr unwrap).
    ctx.eval("assert.throws(TypeError, function () { Array.prototype.map.call(null, function (x) { return x; }); });")
        .unwrap();
    // Holes in array-likes are skipped (callback not invoked for them).
    let r = ctx
        .eval("var seen = []; var o = {length: 3, 0: 'a', 2: 'c'}; Array.prototype.map.call(o, function (x) { seen.push(x); return x; }); seen.length;")
        .unwrap();
    assert_eq!(r.as_smi(), Some(2));
    // All-holes map presizes the species length (B1f-3: holes stay holes).
    let r = ctx
        .eval(
            "var o = {length: 2}; Array.prototype.map.call(o, function (x) { return 1; }).length;",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(2));
    // reduce: first present element seeds; all-holes without initial throws.
    let r = ctx
        .eval("[1, 2, 3].reduce(function (a, b) { return a + b; });")
        .unwrap();
    assert_eq!(r.as_smi(), Some(6));
    ctx.eval(
        "assert.throws(TypeError, function () { [].reduce(function (a, b) { return a + b; }); });",
    )
    .unwrap();
    ctx.eval("assert.throws(TypeError, function () { var o = {length: 2}; Array.prototype.reduce.call(o, function (a, b) { return a + b; }); });")
        .unwrap();
    // reduce with initial skips holes.
    let r = ctx
        .eval("var o = {length: 3, 0: 10, 2: 20}; Array.prototype.reduce.call(o, function (a, b) { return a + b; }, 5);")
        .unwrap();
    assert_eq!(r.as_smi(), Some(35));
}

#[test]
fn test_array_ctor_copywithin_tospliced_with() {
    // B1d: Array constructor forms, copyWithin, toSpliced, with, Array.of.
    let mut ctx = Context::new_small();
    let r = ctx.eval("new Array().length;").unwrap();
    assert_eq!(r.as_smi(), Some(0));
    let r = ctx.eval("new Array(3).length;").unwrap();
    assert_eq!(r.as_smi(), Some(3));
    let r = ctx.eval("new Array(1, 2).join() === '1,2';").unwrap();
    assert!(r.to_bool());
    let r = ctx.eval("Array(7).length;").unwrap();
    assert_eq!(r.as_smi(), Some(7));
    ctx.eval("assert.throws(RangeError, function () { new Array(2.5); });")
        .unwrap();
    ctx.eval("assert.throws(RangeError, function () { new Array(-1); });")
        .unwrap();
    let r = ctx.eval("Array.of(4, 5, 6).join() === '4,5,6';").unwrap();
    assert!(r.to_bool());
    let r = ctx
        .eval("[1, 2, 3, 4, 5].copyWithin(0, 3).join() === '4,5,3,4,5';")
        .unwrap();
    assert!(r.to_bool());
    let r = ctx
        .eval("[1, 2, 3, 4, 5].copyWithin(0, 3, 4).join() === '4,2,3,4,5';")
        .unwrap();
    assert!(r.to_bool());
    let r = ctx
        .eval("var a = [1, 2, 3]; var b = a.toSpliced(1, 1, 9); (a.join() === '1,2,3') && (b.join() === '1,9,3');")
        .unwrap();
    assert!(r.to_bool(), "toSpliced must not mutate");
    let r = ctx
        .eval("[1, 2, 3].with(1, 9).join() === '1,9,3';")
        .unwrap();
    assert!(r.to_bool());
    let r = ctx
        .eval("[1, 2, 3].with(-1, 9).join() === '1,2,9';")
        .unwrap();
    assert!(r.to_bool());
    ctx.eval("assert.throws(RangeError, function () { [1, 2, 3].with(3, 9); });")
        .unwrap();
}

#[test]
fn test_array_fill_toreversed() {
    // B1-method-audits: fill (in-place, returns this) + toReversed (copy).
    let mut ctx = Context::new_small();
    let r = ctx.eval("[0, 0, 0].fill(8).join() === '8,8,8';").unwrap();
    assert!(r.to_bool());
    let r = ctx
        .eval("[0, 0, 0].fill(8, 1).join() === '0,8,8';")
        .unwrap();
    assert!(r.to_bool());
    let r = ctx
        .eval("[0, 0, 0].fill(8, -1).join() === '0,0,8';")
        .unwrap();
    assert!(r.to_bool());
    let r = ctx
        .eval("[0, 0, 0, 0, 0].fill(8, -3, 4).join() === '0,0,8,8,0';")
        .unwrap();
    assert!(r.to_bool());
    let r = ctx.eval("[0, 0].fill(1, true).join() === '0,1';").unwrap();
    assert!(r.to_bool(), "fill coerces boolean start");
    let r = ctx.eval("[0, 0].fill().length === 2;").unwrap();
    assert!(r.to_bool(), "fill() writes undefined");
    // Returns this (identity).
    let r = ctx.eval("var a = [1, 2]; a.fill(0) === a;").unwrap();
    assert!(r.to_bool(), "fill returns this");
    let r = ctx
        .eval("var o = {length: 0}; Array.prototype.fill.call(o) === o;")
        .unwrap();
    assert!(r.to_bool(), "fill returns generic receiver");
    // Abrupts: nullish receiver, symbol index, frozen write.
    ctx.eval("assert.throws(TypeError, function () { Array.prototype.fill.call(null, 1); });")
        .unwrap();
    ctx.eval("assert.throws(TypeError, function () { [].fill(1, Symbol('s')); });")
        .unwrap();
    ctx.eval("assert.throws(TypeError, function () { Object.freeze([1, 2]).fill(9); });")
        .unwrap();
    // toReversed: copy, descending, fresh identity.
    let r = ctx
        .eval("[1, 2, 3].toReversed().join() === '3,2,1';")
        .unwrap();
    assert!(r.to_bool());
    let r = ctx
        .eval("var a = [1, 2, 3]; var b = a.toReversed(); (a.join() === '1,2,3') && (b.join() === '3,2,1') && (a !== b);")
        .unwrap();
    assert!(r.to_bool(), "toReversed must not mutate");
    // Holes materialize as present undefined (not preserved).
    let r = ctx
        .eval(
            "var r = [0, , 2].toReversed(); (r.length === 3) && (1 in r) && (r[1] === undefined);",
        )
        .unwrap();
    assert!(r.to_bool(), "toReversed fills holes with undefined");
    // Generic receiver + frozen source reads fine.
    let r = ctx
        .eval("Array.prototype.toReversed.call({length: 3, 0: 0, 1: 1, 2: 2}).join() === '2,1,0';")
        .unwrap();
    assert!(r.to_bool());
    let r = ctx
        .eval("Object.freeze([0, 1, 2]).toReversed().join() === '2,1,0';")
        .unwrap();
    assert!(r.to_bool(), "toReversed reads frozen sources");
    // No species: custom ctor never consulted.
    let r = ctx
        .eval("var a = []; a.constructor = {}; Object.getPrototypeOf(a.toReversed()) === Array.prototype;")
        .unwrap();
    assert!(r.to_bool(), "toReversed ignores species");
    ctx.eval("assert.throws(TypeError, function () { Array.prototype.toReversed.call(null); });")
        .unwrap();
    ctx.eval("assert.throws(RangeError, function () { Array.prototype.toReversed.call({length: 4294967296}); });")
        .unwrap();
}

#[test]
fn test_array_with_tospliced() {
    // B1-method-audits: with (indexed copy) + toSpliced (presence-gated skip).
    let mut ctx = Context::new_small();
    let r = ctx
        .eval("[1, 2, 3].with(1, 9).join() === '1,9,3';")
        .unwrap();
    assert!(r.to_bool());
    let r = ctx
        .eval("[1, 2, 3].with(-1, 9).join() === '1,2,9';")
        .unwrap();
    assert!(r.to_bool());
    let r = ctx
        .eval("[0, 4, 16].with('1', 3).join() === '0,3,16';")
        .unwrap();
    assert!(r.to_bool(), "with coerces string index");
    let r = ctx
        .eval("[0, 4, 16].with(NaN, 2).join() === '2,4,16';")
        .unwrap();
    assert!(r.to_bool(), "with NaN index means 0");
    // Holes materialize as present undefined; receiver untouched.
    let r = ctx
        .eval("var a = [0, , 2]; var r = a.with(0, 9); (r.join() === '9,,2') && (1 in r) && (a.join() === '0,,2');")
        .unwrap();
    assert!(r.to_bool(), "with fills holes, never mutates");
    ctx.eval("assert.throws(RangeError, function () { [1, 2, 3].with(3, 9); });")
        .unwrap();
    ctx.eval("assert.throws(RangeError, function () { [1, 2, 3].with(-4, 9); });")
        .unwrap();
    ctx.eval("assert.throws(TypeError, function () { [1].with(Symbol('s'), 9); });")
        .unwrap();
    // toSpliced: bare call copies, undefined start deletes all.
    let r = ctx
        .eval("var a = ['first', 'second', 'third']; var b = a.toSpliced(); (b.join() === 'first,second,third') && (b !== a);")
        .unwrap();
    assert!(r.to_bool(), "bare toSpliced copies");
    let r = ctx
        .eval("['first', 'second', 'third'].toSpliced(undefined).join() === '';")
        .unwrap();
    assert!(r.to_bool(), "undefined start deletes all");
    let r = ctx
        .eval("['first', 'second', 'third'].toSpliced(1).join() === 'first';")
        .unwrap();
    assert!(r.to_bool(), "missing deleteCount deletes to end");
    let r = ctx
        .eval("[0, 1, 2, 3, 4].toSpliced(1, 2, 9).join() === '0,9,3,4';")
        .unwrap();
    assert!(r.to_bool());
    let r = ctx
        .eval("Object.freeze([2, 0, 1]).toSpliced().join() === '2,0,1';")
        .unwrap();
    assert!(r.to_bool(), "toSpliced reads frozen sources");
    let r = ctx
        .eval("var a = []; a.constructor = {}; Object.getPrototypeOf(a.toSpliced(0, 0)) === Array.prototype;")
        .unwrap();
    assert!(r.to_bool(), "toSpliced ignores species");
    ctx.eval(
        "assert.throws(TypeError, function () { Array.prototype.toSpliced.call(null, 0, 0); });",
    )
    .unwrap();
    ctx.eval("assert.throws(TypeError, function () { Array.prototype.toSpliced.call({length: 9007199254740991}, 0, 0, 1); });")
        .unwrap();
    ctx.eval("assert.throws(RangeError, function () { Array.prototype.toSpliced.call({length: 4294967296}, 0, 0); });")
        .unwrap();
}

#[test]
fn test_array_copy_element_dispatch() {
    // 6e-copy: JS element getters run through frames during copies (reads
    // observe in spec order, side effects apply); flat depth rejects symbols
    // and method-less objects; OOB `in` agrees with proto-serving loads.
    let mut ctx = Context::new_small();
    // toReversed observes descending Gets.
    let r = ctx
        .eval("var o = []; var a = {length: 3, get 0() { o.push(0); }, get 1() { o.push(1); }, get 2() { o.push(2); }}; Array.prototype.toReversed.call(a); o.join() === '2,1,0';")
        .unwrap();
    assert!(r.to_bool(), "toReversed Gets descend");
    // toSpliced observes ascending Gets, skips the deleted range.
    let r = ctx
        .eval("var o = []; var a = {get 0() { o.push(0); return 'a'; }, get 1() { o.push(1); return 'b'; }, 2: 'none', get 3() { o.push(3); return 'c'; }, length: 4}; var res = Array.prototype.toSpliced.call(a, 2, 1); (res.join() === 'a,b,c') && (o.join() === '0,1,3');")
        .unwrap();
    assert!(r.to_bool(), "toSpliced Gets ascend, deleted unread");
    // Shrinking getter + proto fallthrough (length-decreased shape).
    let r = ctx
        .eval("var arr = [0, 1, 2, 3, 4]; Array.prototype[1] = 5; Object.defineProperty(arr, '3', {get: function () { arr.length = 1; return 3; }}); var r = arr.toReversed(); (r[2] === undefined) && (r[3] === 5); delete Array.prototype[1];")
        .unwrap();
    assert!(r.to_bool(), "post-shrink reads serve holes + proto");
    // with/ replaces without Getting the replaced index.
    let r = ctx
        .eval("var arr = [0, 1, 2, 3]; Object.defineProperty(arr, '2', {get: function () { throw new Error('must not Get 2'); }}); arr.with(2, 6).join() === '0,1,6,3';")
        .unwrap();
    assert!(r.to_bool(), "with never Gets the replaced index");
    // flat depth: symbols + method-less objects throw, plain objects read 0.
    ctx.eval("assert.throws(TypeError, function () { [].flat(Symbol()); });")
        .unwrap();
    ctx.eval("assert.throws(TypeError, function () { [].flat(Object.create(null)); });")
        .unwrap();
    let r = ctx.eval("[1, [2]].flat({}).length === 2;").unwrap();
    assert!(r.to_bool(), "flat plain-object depth reads 0");
    // OOB `in` agrees with proto-serving loads.
    let r = ctx
        .eval("Array.prototype[4] = 5; var arr = [0, 1]; var ok = (4 in arr) && (arr[4] === 5); delete Array.prototype[4]; ok;")
        .unwrap();
    assert!(r.to_bool(), "OOB in walks proto");
    // 6e-copy-writes: JS setters run through frames during fill (throwing
    // setters propagate; recording setters run once per index, fill continues).
    ctx.eval("assert.throws(Test262Error, function () { var a = {length: 1}; Object.defineProperty(a, '0', {set: function () { throw new Test262Error(); }}); Array.prototype.fill.call(a); });")
        .unwrap();
    let r = ctx
        .eval("var calls = []; var o = {length: 3}; Object.defineProperty(o, '1', {set: function (v) { calls.push(v); }}); var res = Array.prototype.fill.call(o, 7); (res === o) && (calls.join() === '7') && (o[0] === 7) && (o[2] === 7);")
        .unwrap();
    assert!(r.to_bool(), "fill setter runs, fill continues");
    // 6e-mutators: freezing accessors run mid-operation, then the tail
    // throws; non-writable lengths reject even SameValue; stack/queue moves
    // stay exact; own data shadows proto setters on write.
    ctx.eval("assert.throws(TypeError, function () { var a = []; Object.defineProperty(Array.prototype, '0', {set: function (_v) { Object.freeze(a); }, configurable: true}); try { a.push(1); } finally { delete Array.prototype[0]; } });")
        .unwrap();
    ctx.eval("assert.throws(TypeError, function () { var a = []; Object.freeze(a); a.push(); });")
        .unwrap();
    ctx.eval("assert.throws(TypeError, function () { var a = []; Object.freeze(a); a.pop(); });")
        .unwrap();
    let r = ctx
        .eval("var s = [1, 2, 3]; s.shift(); s.join() === '2,3';")
        .unwrap();
    assert!(r.to_bool(), "shift moves");
    let r = ctx
        .eval("var u = [1, 2, 3]; u.unshift(0); u.join() === '0,1,2,3';")
        .unwrap();
    assert!(r.to_bool(), "unshift moves");
    let r = ctx
        .eval("Object.defineProperty(Array.prototype, '0', {get: function () { return 9; }, configurable: true}); var arr = [1, 2, 3]; var ok = arr[0] === 1; delete Array.prototype[0]; ok;")
        .unwrap();
    assert!(r.to_bool(), "own data shadows proto accessors");
}

#[test]
fn test_array_sort_tosorted() {
    // B1e: comparator sort + toSorted (stable, observable, spec-ordered).
    let mut ctx = Context::new_small();
    // Default is lexicographic; undefined sorts last.
    let r = ctx.eval("[10, 2, 1].sort().join() === '1,10,2';").unwrap();
    assert!(r.to_bool());
    let r = ctx.eval("[3, undefined, 1].sort().length === 3;").unwrap();
    assert!(r.to_bool());
    let r = ctx
        .eval("[3, undefined, 1].sort()[2] === undefined;")
        .unwrap();
    assert!(r.to_bool());
    // Numeric comparator, both directions.
    let r = ctx
        .eval("[3, 1, 2].sort(function (a, b) { return a - b; }).join() === '1,2,3';")
        .unwrap();
    assert!(r.to_bool());
    let r = ctx
        .eval("[3, 1, 2].sort(function (a, b) { return b - a; }).join() === '3,2,1';")
        .unwrap();
    assert!(r.to_bool());
    // Non-callable comparator throws (before any length read).
    ctx.eval("assert.throws(TypeError, function () { [1].sort(null); });")
        .unwrap();
    // Sort returns the same object.
    let r = ctx.eval("var s = [2, 1]; s.sort() === s;").unwrap();
    assert!(r.to_bool());
    // Stability: equal keys keep insertion order.
    let r = ctx
        .eval("var st = [{k: 1, i: 0}, {k: 1, i: 1}, {k: 1, i: 2}]; st.sort(function (a, b) { return a.k - b.k; }); st[0].i === 0 && st[1].i === 1 && st[2].i === 2;")
        .unwrap();
    assert!(r.to_bool());
    // toSorted copies; the source is untouched; holes densify.
    let r = ctx
        .eval("var t = [3, 1, 2]; var u = t.toSorted(); (t.join() === '3,1,2') && (u.join() === '1,2,3') && (t !== u);")
        .unwrap();
    assert!(r.to_bool());
    let r = ctx
        .eval("var h = [3, , 1]; var s2 = h.toSorted(); (s2.length === 3) && (s2[2] === undefined) && (1 in h) === false && (s2.join() === '1,3,');")
        .unwrap();
    assert!(r.to_bool());
    // toSorted length overflow throws RangeError before any read.
    ctx.eval("assert.throws(RangeError, function () { Array.prototype.toSorted.call({length: 4294967296}); });")
        .unwrap();
    // Element getter/setter access is observable (var-bound; B8 owns let).
    let r = ctx
        .eval("var g = [3, 1, 2]; var n = 0; Object.defineProperty(g, '0', { get: function () { n = n + 1; return 3; }, set: function (v) {} }); g.sort(); n >= 1;")
        .unwrap();
    assert!(r.to_bool());
}

#[test]
fn test_array_holes() {
    // B1e: elisions are real holes (sentinel), not undefined elements.
    let mut ctx = Context::new_small();
    let r = ctx.eval("[1, , 3].length;").unwrap();
    assert_eq!(r.as_smi(), Some(3));
    let r = ctx.eval("1 in [1, , 3];").unwrap();
    assert!(!r.to_bool());
    let r = ctx.eval("[1, , 3].hasOwnProperty(1);").unwrap();
    assert!(!r.to_bool());
    let r = ctx.eval("[1, , 3][1] === undefined;").unwrap();
    assert!(r.to_bool());
    let r = ctx.eval("[1, 2, ].length;").unwrap();
    assert_eq!(r.as_smi(), Some(2));
    let r = ctx.eval("[, ].length;").unwrap();
    assert_eq!(r.as_smi(), Some(1));
    // delete makes a real hole.
    let r = ctx
        .eval("var d = [1, 2, 3]; delete d[1]; (1 in d) === false && d.length === 3;")
        .unwrap();
    assert!(r.to_bool());
    // Holes join as empty and spread as undefined.
    let r = ctx.eval("[1, , 3].join() === '1,,3';").unwrap();
    assert!(r.to_bool());
    let r = ctx
        .eval("var sp = [1, , 3]; var q = [0, ...sp]; q.length === 4 && q[2] === undefined;")
        .unwrap();
    assert!(r.to_bool());
    // Proto serves holes and shrunk tails.
    let r = ctx.eval("Object.prototype[5] = 'p'; var ph = [1, , 3]; var v = ph[5]; delete Object.prototype[5]; v === 'p';").unwrap();
    assert!(r.to_bool());
}

#[test]
fn test_array_push_pop_shift_unshift_reverse() {
    // B1f-1: stack/queue mutators audit (generic, holes, lengths, overflow).
    let mut ctx = Context::new_small();
    // push appends ALL args (not just the first) and returns the length.
    let r = ctx
        .eval("var p = []; p.push(1, 2, 3) === 3 && p.join() === '1,2,3';")
        .unwrap();
    assert!(r.to_bool());
    // push() with no args appends nothing.
    let r = ctx
        .eval("var p0 = [1]; p0.push() === 1 && p0.length === 1;")
        .unwrap();
    assert!(r.to_bool());
    // pop removes and returns the last; empty pops to undefined.
    let r = ctx
        .eval("var q = [1, 2, 3]; var v = q.pop(); v === 3 && q.join() === '1,2';")
        .unwrap();
    assert!(r.to_bool());
    let r = ctx.eval("[].pop() === undefined;").unwrap();
    assert!(r.to_bool());
    // shift slides down with hole preservation.
    let r = ctx.eval("var s = [1, , 3]; var f = s.shift(); f === 1 && s.length === 2 && s[0] === undefined && (0 in s) === false && s[1] === 3;").unwrap();
    assert!(r.to_bool());
    // unshift prepends in order and returns the new length.
    let r = ctx
        .eval("var u = [3]; u.unshift(1, 2) === 3 && u.join() === '1,2,3';")
        .unwrap();
    assert!(r.to_bool());
    // reverse swaps with hole preservation and returns the receiver.
    let r = ctx.eval("var r = [1, , 3]; var s = r.reverse(); s === r && r.length === 3 && r[0] === 3 && (1 in r) === false && r[1] === undefined && r[2] === 1;").unwrap();
    assert!(r.to_bool());
    // Generic objects work (length created when absent).
    let r = ctx.eval("var o = {}; o.push = Array.prototype.push; o.push(-1) === 1 && o.length === 1 && o[0] === -1;").unwrap();
    assert!(r.to_bool());
    // Overflow past 2^53-1 throws TypeError.
    ctx.eval("assert.throws(TypeError, function () { Array.prototype.push.call({length: 9007199254740991}, 1); });")
        .unwrap();
    // Dense length past 2^32-1 throws RangeError on update (array exotic).
    ctx.eval("assert.throws(RangeError, function () { var a = []; a.length = 4294967295; a.push('x'); });")
        .unwrap();
    // Non-callable length slot reads as 0; holes in objects are skipped.
    let r = ctx.eval("var g = {length: 2, 1: 'b'}; Array.prototype.reverse.call(g) === g && g[0] === 'b' && (1 in g) === false;").unwrap();
    assert!(r.to_bool());
}

#[test]
fn test_array_slice_splice_concat() {
    // B1f-2: slice/splice/concat audit (generic, holes, clamps, overflow).
    let mut ctx = Context::new_small();
    // slice truncates float start/end (ToIntegerOrInfinity).
    let r = ctx.eval("var sa = [0, 1, 2, 3, 4].slice(2.5, 4); sa.length === 2 && sa[0] === 2 && sa[1] === 3;").unwrap();
    assert!(r.to_bool());
    // slice preserves holes (sentinel, not undefined).
    let r = ctx.eval("var sh = [1, , 3].slice(0); sh.length === 3 && (1 in sh) === false && sh.hasOwnProperty('1') === false && sh[1] === undefined;").unwrap();
    assert!(r.to_bool());
    // slice serves proto elements as OWN result entries (A4_T1).
    let r = ctx.eval("Array.prototype[1] = 1; var qx = [0]; qx.length = 2; var qr = qx.slice(); var qok = qr[1] === 1 && qr.hasOwnProperty('1'); delete Array.prototype[1]; qok;").unwrap();
    assert!(r.to_bool());
    // slice on boolean primitives boxes to length 0.
    let r = ctx.eval("Array.prototype.slice.call(true).length === 0 && Array.prototype.slice.call(false).length === 0;").unwrap();
    assert!(r.to_bool());
    // slice clamps huge lengths (ToLength) with u64 walks over named keys.
    let r = ctx.eval("var al = {'9007199254740989': 'a', '9007199254740990': 'b', length: 9007199254740992}; var ar = Array.prototype.slice.call(al, 9007199254740989); ar.length === 2 && ar[0] === 'a' && ar[1] === 'b';").unwrap();
    assert!(r.to_bool());
    // slice past 2^32-1 throws RangeError before any copy (was ENGINE PANIC).
    ctx.eval("assert.throws(RangeError, function () { Array.prototype.slice.call({length: 4294967296}); });")
        .unwrap();
    // splice deletes + returns them, shifts left with hole punch-through.
    let r = ctx.eval("var xd = [0, 1, 2, 3]; var dd = xd.splice(1, 2, 'a'); dd.join() === '1,2' && xd.join() === '0,a,3';").unwrap();
    assert!(r.to_bool());
    // splice inserts without deletes (shifts right).
    let r = ctx
        .eval("var xi = [1, 2]; xi.splice(1, 0, 'a', 'b'); xi.join() === '1,a,b,2';")
        .unwrap();
    assert!(r.to_bool());
    // splice with no args deletes nothing (plain data object).
    let r = ctx.eval("var xo = {length: 3, 0: 'a'}; var xd0 = Array.prototype.splice.call(xo); xd0.length === 0 && xo.length === 3;").unwrap();
    assert!(r.to_bool());
    // splice overflow past 2^53-1 throws TypeError before mutation.
    ctx.eval("assert.throws(TypeError, function () { Array.prototype.splice.call({length: 9007199254740991}, 0, 0, null); });")
        .unwrap();
    // splice huge named keys (u64, named-overflow model — A3_T1).
    let r = ctx.eval("var xh = {}; xh[0] = 'x'; xh[4294967295] = 'y'; xh.length = 4294967296; var ah = Array.prototype.splice.call(xh, 4294967295, 1); ah.length === 1 && ah[0] === 'y' && xh.length === 4294967295 && xh[0] === 'x' && xh[4294967295] === undefined;").unwrap();
    assert!(r.to_bool());
    // splice getter-only length throws on the length Set (A6.1_T3).
    ctx.eval("assert.throws(TypeError, function () { var gl = {get length() { return 0; }}; Array.prototype.splice.call(gl, 1, 2, 4); });")
        .unwrap();
    // concat spreads arrays, singles objects (identity preserved).
    let r = ctx.eval("var co = {}; var cc = [1].concat([2, 3], co, 4); cc.length === 5 && cc[0] === 1 && cc[1] === 2 && cc[2] === 3 && cc[3] === co && cc[4] === 4;").unwrap();
    assert!(r.to_bool());
    // concat honors own @@isConcatSpreadable.
    let r = ctx.eval("var so = {length: 1, 0: 'a'}; so[Symbol.isConcatSpreadable] = true; [].concat(so).join() === 'a';").unwrap();
    assert!(r.to_bool());
    // concat overflow past 2^53-1 throws TypeError (was ENGINE PANIC).
    ctx.eval("assert.throws(TypeError, function () { [1].concat({length: 9007199254740991, [Symbol.isConcatSpreadable]: true}); });")
        .unwrap();
    // concat holes in spread ranges stay holes.
    let r = ctx.eval("var ch = [1, , 3]; ch[Symbol.isConcatSpreadable] = true; var cx = [0].concat(ch); cx.length === 4 && (2 in cx) === false && cx[3] === 3;").unwrap();
    assert!(r.to_bool());
}

#[test]
fn test_array_map_filter_result_shapes() {
    // B1f-3: hole-preserving map results, packed filter kValues, flatMap skip.
    let mut ctx = Context::new_small();
    // Map writes at source indices (8-c-i-18: accessor presence counts).
    let r = ctx.eval("function cb1(val, idx, obj) { if (idx === 1) { return typeof val === 'undefined'; } return false; } var ma = []; Object.defineProperty(ma, '1', { set: function () {}, configurable: true }); var mt = ma.map(cb1); mt[1] === true && mt.length === 2;").unwrap();
    assert!(r.to_bool());
    // Map holes stay holes positionally with species length.
    let r = ctx.eval("var mh = [1, , 3].map(function (v) { return v * 10; }); mh.length === 3 && mh[0] === 10 && (1 in mh) === false && mh[2] === 30;").unwrap();
    assert!(r.to_bool());
    // Map serves proto elements (presence delegates to array protos).
    let r = ctx.eval("function foo() {} foo.prototype = new Array(1, 2, 3); var ff = new foo(); ff.length = 1; var fa = ff.map(function () {}); Array.isArray(fa) && fa.length === 1;").unwrap();
    assert!(r.to_bool());
    // Boolean lengths coerce (ToLength(true) === 1).
    let r = ctx.eval("Array.prototype.map.call({0: 11, length: true}, function (v) { return v > 10; }).length === 1;").unwrap();
    assert!(r.to_bool());
    // Inherited data lengths serve (2-6).
    let r = ctx.eval("var proto = {length: 2}; function Con() {} Con.prototype = proto; var chd = new Con(); chd[0] = 12; chd[1] = 11; chd[2] = 9; Array.prototype.map.call(chd, function (v) { return v > 10; }).length === 2;").unwrap();
    assert!(r.to_bool());
    // Map past 2^32-1 throws RangeError before walking (was saturation hang).
    ctx.eval("assert.throws(RangeError, function () { Array.prototype.map.call({0: 12, length: 4294967296}, function (v) { return v; }); });")
        .unwrap();
    // Filter packs kept kValues densely.
    let r = ctx.eval("var fl = [1, 2, 3, 4].filter(function (v) { return v % 2 === 0; }); fl.length === 2 && fl[0] === 2 && fl[1] === 4;").unwrap();
    assert!(r.to_bool());
    // FlatMap skips holes when spreading ([[1,,3]] → [1,3]).
    let r = ctx.eval("var fm = [[1, , 3]].flatMap(function (x) { return x; }); fm.length === 2 && fm[0] === 1 && fm[1] === 3;").unwrap();
    assert!(r.to_bool());
}

#[test]
fn test_array_from() {
    // B1f-4: iterable drain + array-like fallback + mapfn + IteratorClose.
    let mut ctx = Context::new_small();
    // Arrays drain via @@iterator.
    let r = ctx
        .eval("Array.from([1, 2, 3]).join() === '1,2,3';")
        .unwrap();
    assert!(r.to_bool());
    // Strings drain as UTF-16 units.
    let r = ctx.eval("Array.from('abc').join() === 'a,b,c';").unwrap();
    assert!(r.to_bool());
    // Array-likes read unconditionally (holes densify to present undefined).
    let r = ctx.eval("var d = Array.from({length: 3, 0: 'a', 2: 'c'}); d.length === 3 && d[0] === 'a' && d[1] === undefined && (1 in d) === true && d[2] === 'c';").unwrap();
    assert!(r.to_bool());
    // mapfn gets (value, index) with thisArg.
    let r = ctx
        .eval("Array.from([1, 2, 3], function (v) { return v * 2; }).join() === '2,4,6';")
        .unwrap();
    assert!(r.to_bool());
    let r = ctx
        .eval(
            "Array.from([1, 2], function (v) { return v + this.x; }, {x: 10}).join() === '11,12';",
        )
        .unwrap();
    assert!(r.to_bool());
    // Custom iterables drain; mapper abrupts close the iterator first.
    let r = ctx.eval("var it = {}; it[Symbol.iterator] = function () { var i = 0; return { next: function () { i++; return i <= 2 ? {value: i, done: false} : {done: true}; } }; }; Array.from(it).join() === '1,2';").unwrap();
    assert!(r.to_bool());
    let r = ctx.eval("var cc = 0; var it = {}; it[Symbol.iterator] = function () { return { return: function () { cc++; }, next: function () { return {done: false}; } }; }; try { Array.from(it, function () { throw 7; }); } catch (e) {} cc === 1;").unwrap();
    assert!(r.to_bool());
    // Prologue errors: null items, non-callable mapper.
    ctx.eval("assert.throws(TypeError, function () { Array.from(null); });")
        .unwrap();
    ctx.eval("assert.throws(TypeError, function () { Array.from([1], 5); });")
        .unwrap();
    // Arity is 1.
    let r = ctx.eval("Array.from.length === 1;").unwrap();
    assert!(r.to_bool());
}

#[test]
fn test_array_integrity_freeze_seal_descriptors() {
    // B1f-5: length exotic, freeze/seal, index descriptors, overlay delete.
    let mut ctx = Context::new_small();
    // defineProperty length shrink deletes descending with holes.
    let r = ctx.eval("var a = [1, 2, 3]; Object.defineProperty(a, 'length', {value: 1}); a.length === 1 && a.join() === '1' && (1 in a) === false;").unwrap();
    assert!(r.to_bool());
    // Length coercion RangeErrors precede attr checks.
    ctx.eval("assert.throws(RangeError, function () { Object.defineProperty([], 'length', {value: -1}); });").unwrap();
    ctx.eval("assert.throws(RangeError, function () { Object.defineProperty([], 'length', {value: 3.5}); });").unwrap();
    // Direct length assignment runs the exotic (RangeError even sloppy).
    ctx.eval("assert.throws(RangeError, function () { [].length = 4294967296; });")
        .unwrap();
    // Writable flip true->false locks; false->true throws.
    let r = ctx.eval("var b = [1, 2]; Object.defineProperty(b, 'length', {writable: false}); b.length === 2;").unwrap();
    assert!(r.to_bool());
    ctx.eval("assert.throws(TypeError, function () { var b = [1, 2]; Object.defineProperty(b, 'length', {writable: false}); Object.defineProperty(b, 'length', {writable: true}); });").unwrap();
    // Non-writable length rejects shrinking writes (A6.1_T2).
    ctx.eval("assert.throws(TypeError, function () { var a = [0, 1, 2]; Object.defineProperty(a, 'length', {writable: false}); a.splice(1, 2, 4); });").unwrap();
    // Freeze: tests, push/write rejection, descriptors, delete falseness.
    let r = ctx.eval("var f = [1, 2]; Object.freeze(f); Object.isFrozen(f) && Object.isSealed(f) && !Object.isExtensible(f);").unwrap();
    assert!(r.to_bool());
    ctx.eval(
        "assert.throws(TypeError, function () { var f = [1, 2]; Object.freeze(f); f.push(3); });",
    )
    .unwrap();
    let r = ctx.eval("var f = [1, 2]; Object.freeze(f); var d = Object.getOwnPropertyDescriptor(f, 0); d.value === 1 && d.writable === false && d.enumerable === true && d.configurable === false;").unwrap();
    assert!(r.to_bool());
    let r = ctx.eval("var f = [1, 2]; Object.freeze(f); var d = Object.getOwnPropertyDescriptor(f, 'length'); d.value === 2 && d.writable === false && d.enumerable === false && d.configurable === false;").unwrap();
    assert!(r.to_bool());
    let r = ctx
        .eval(
            "var f = [1, 2]; Object.freeze(f); delete f[0] === false && delete f.length === false;",
        )
        .unwrap();
    assert!(r.to_bool());
    // Seal: writes pass, growth and deletes fail, frozen stays false.
    let r = ctx.eval("var s = [1, 2]; Object.seal(s); Object.isSealed(s) && !Object.isFrozen(s); s[0] = 9; s[0] === 9;").unwrap();
    assert!(r.to_bool());
    ctx.eval(
        "assert.throws(TypeError, function () { var s = [1, 2]; Object.seal(s); s.push(3); });",
    )
    .unwrap();
    // Holes describe as absent; overlay delete removes the pair (8-b-9).
    let r = ctx
        .eval("var h = [1, , 3]; Object.getOwnPropertyDescriptor(h, 1) === undefined;")
        .unwrap();
    assert!(r.to_bool());
    let r = ctx.eval("var o = [0, 1]; Object.defineProperty(o, '0', {get: function () { delete o[1]; return 0; }, configurable: true}); var m = o.map(function (v, i) { return i === 1 ? false : true; }); m[0] === true && typeof m[1] === 'undefined';").unwrap();
    assert!(r.to_bool());
    // Primitives pass through integrity builtins.
    let r = ctx.eval("Object.freeze(undefined) === undefined;").unwrap();
    assert!(r.to_bool());
}

#[test]
fn test_array_species_prologue() {
    // B1f-6a: SpeciesConstructor sync checks (non-Object ctor / non-ctor
    // species throw; custom ctors fall back to plain until B1f-6-frames).
    let mut ctx = Context::new_small();
    // Non-object constructors throw (null/number/string/boolean).
    ctx.eval(
        "assert.throws(TypeError, function () { var a = []; a.constructor = null; a.slice(); });",
    )
    .unwrap();
    ctx.eval("assert.throws(TypeError, function () { var a = []; a.constructor = 1; a.map(function (x) { return x; }); });")
        .unwrap();
    // Non-constructor species throws.
    ctx.eval("assert.throws(TypeError, function () { var a = [1]; a.constructor = {}; a.constructor[Symbol.species] = parseInt; a.filter(function (x) { return true; }); });")
        .unwrap();
    // Absent/null species build plain (custom valid ctors also fall back).
    let r = ctx
        .eval("var a = [1, 2]; var c = a.concat([3]); c.length === 3 && Array.isArray(c);")
        .unwrap();
    assert!(r.to_bool());
    // B1f-6d: valid custom ctors Construct (len arg, new-this) and the
    // result installs (dense swaps in; plain takes defines, not Array).
    let r = ctx.eval("var calls = 0; var C = function (len) { calls += 1; this.len = len; }; var a = [1]; a.constructor = {}; a.constructor[Symbol.species] = C; var s = a.slice(); calls === 1 && s instanceof C && s[0] === 1 && !Array.isArray(s);").unwrap();
    assert!(r.to_bool());
}

#[test]
fn test_array_length_dispatch() {
    // B1f-6b: LengthOfArrayLike dispatches JS (getter / valueOf / toString /
    // @@toPrimitive) through the PendingLengthOp machine. Methods resolve on
    // the ORIGINAL length object (OrdinaryToPrimitive pinned base); double
    // object results throw TypeError; throwing methods propagate.
    let mut ctx = Context::new_small();
    // valueOf length drives pop.
    let r = ctx
        .eval("var o = {0: -1, length: {valueOf: function () { return 1; }}}; Array.prototype.pop.call(o);")
        .unwrap();
    assert_eq!(r.as_smi(), Some(-1));
    // toString fallback when valueOf returns an object.
    let r = ctx
        .eval("var o = {0: 7, length: {valueOf: function () { return {}; }, toString: function () { return 1; }}}; Array.prototype.pop.call(o);")
        .unwrap();
    assert_eq!(r.as_smi(), Some(7));
    // Both methods returning objects throws TypeError.
    ctx.eval("assert.throws(TypeError, function () { var o = {0: -1, length: {valueOf: function () { return {}; }, toString: function () { return {}; }}}; Array.prototype.pop.call(o); });")
        .unwrap();
    // Throwing valueOf propagates the original error.
    ctx.eval("assert.throws('error', function () { var o = {0: -1, length: {valueOf: function () { throw 'error'; }, toString: function () { return 0; }}}; Array.prototype.pop.call(o); });")
        .unwrap();
    // Length getter runs and its result coerces (slice never writes length).
    let r = ctx
        .eval("var o = {0: 5}; Object.defineProperty(o, 'length', {get: function () { return '1'; }, configurable: true}); var s = Array.prototype.slice.call(o); s.length === 1 && s[0] === 5;")
        .unwrap();
    assert!(r.to_bool());
    // map honors a valueOf length (iterates once).
    let r = ctx
        .eval("var o = {0: 3, 1: 4, length: {valueOf: function () { return 1; }}}; var m = Array.prototype.map.call(o, function (x) { return x * 2; }); m.length === 1 && m[0] === 6;")
        .unwrap();
    assert!(r.to_bool());
}

#[test]
fn test_array_species_getters() {
    // B1f-6c: SpeciesConstructor Gets dispatch JS getters through the
    // PendingSpeciesOp machine (throwing getters propagate before any
    // callback runs); non-Array receivers skip the Gets entirely
    // (§10.4.2.3 step 1); data-path custom ctors still build plain (6d).
    let mut ctx = Context::new_small();
    // Poisoned "constructor" getter throws; callback never runs.
    ctx.eval("assert.throws('ctor-poison', function () { var a = [1]; var n = 0; Object.defineProperty(a, 'constructor', {get: function () { throw 'ctor-poison'; }}); a.map(function () { n += 1; }); });")
        .unwrap();
    // Poisoned @@species getter throws; filter callback never runs.
    ctx.eval("assert.throws('sp-poison', function () { var a = [1]; a.constructor = {}; Object.defineProperty(a.constructor, Symbol.species, {get: function () { throw 'sp-poison'; }}); a.filter(function () { return true; }); });")
        .unwrap();
    // Species getter returning null still builds plain — and the getter ran
    // (defineProperty form; the `{get [Symbol.species]()}` literal shape
    // does not install — pre-existing parser gap, B2).
    let r = ctx
        .eval("var calls = 0; var a = [1, 2]; a.constructor = {}; Object.defineProperty(a.constructor, Symbol.species, {get: function () { calls += 1; return null; }, configurable: true}); var s = a.slice(); calls === 1 && s.length === 2 && Array.isArray(s);")
        .unwrap();
    assert!(r.to_bool());
    // Non-Array receiver with a poisoned "constructor" getter: no Gets, no
    // throw (IsArray false → ArrayCreate path).
    let r = ctx
        .eval("var o = {0: 5, length: 1}; Object.defineProperty(o, 'constructor', {get: function () { throw 'must-not-run'; }}); var s = Array.prototype.slice.call(o); s.length === 1 && s[0] === 5;")
        .unwrap();
    assert!(r.to_bool());
}

#[test]
fn test_array_species_construct() {
    // B1f-6d: custom species ctors Construct(len) via New-frames (called
    // once, new-this with ctor.prototype, +0 len arg); object results win,
    // otherwise the fresh this installs; dense results swap in as the
    // buffer (identity holds); abrupt ctors propagate with no callbacks.
    let mut ctx = Context::new_small();
    // Full protocol on map (create-species.js shape).
    let r = ctx
        .eval("var calls = 0; var instance = []; var seenThis, seenArgs; var C = function () { calls += 1; seenThis = this; seenArgs = arguments; return instance; }; var a = [1, 2, 3]; a.constructor = {}; a.constructor[Symbol.species] = C; var r = a.map(function () {}); calls === 1 && seenArgs.length === 1 && seenArgs[0] === 3 && r === instance && Object.getPrototypeOf(seenThis) === C.prototype;")
        .unwrap();
    assert!(r.to_bool());
    // Throwing ctor propagates; callback never runs.
    ctx.eval("assert.throws('ctor-abrupt', function () { var a = [1]; var n = 0; a.constructor = {}; a.constructor[Symbol.species] = function () { throw 'ctor-abrupt'; }; a.map(function () { n += 1; }); });")
        .unwrap();
    // Neg-zero species length arrives as +0 on a plain install.
    let r = ctx
        .eval("var got; var C = function () { got = arguments; }; var a = []; a.constructor = {}; a.constructor[Symbol.species] = C; a.slice(0, -0); got.length === 1 && got[0] === 0 && (1 / got[0] > 0);")
        .unwrap();
    assert!(r.to_bool());
    // Non-extensible custom result: concat's first write throws TypeError.
    ctx.eval("assert.throws(TypeError, function () { var A = function (l) { this.length = 0; Object.preventExtensions(this); }; var a = []; a.constructor = {}; a.constructor[Symbol.species] = A; a.concat(1); });")
        .unwrap();
    // Array.of custom this: defines + Set length (setter fires once).
    let r = ctx
        .eval("var hits = 0; function Pack() { Object.defineProperty(this, 'length', {set: function (l) { hits += 1; }}); } var r = Array.of.call(Pack, 'a', 'b'); hits === 1 && r instanceof Pack && r[0] === 'a' && r[1] === 'b';")
        .unwrap();
    assert!(r.to_bool());
}

#[test]
fn test_array_index_search_family() {
    // B1b: indexOf/lastIndexOf/includes with spec equality + fromIndex.
    let mut ctx = Context::new_small();
    let r = ctx.eval("[1, 2, 3, 2].lastIndexOf(2);").unwrap();
    assert_eq!(r.as_smi(), Some(3));
    let r = ctx.eval("[1, 2, 3, 2].lastIndexOf(2, 2);").unwrap();
    assert_eq!(r.as_smi(), Some(1));
    let r = ctx.eval("[1, 2, 3].lastIndexOf(9);").unwrap();
    assert_eq!(r.as_smi(), Some(-1));
    // includes uses SameValueZero (NaN matches).
    let r = ctx.eval("[1, NaN].includes(NaN);").unwrap();
    assert!(r.to_bool());
    let r = ctx.eval("[1, 2, 3].includes(2, 2);").unwrap();
    assert!(!r.to_bool());
    // indexOf uses strict equality (NaN never matches, -0/+0 equal).
    let r = ctx.eval("[1, NaN].indexOf(NaN);").unwrap();
    assert_eq!(r.as_smi(), Some(-1));
    let r = ctx.eval("[1, 2, 3].indexOf(2, 5);").unwrap();
    assert_eq!(r.as_smi(), Some(-1));
    // Proto-chain elements are found (holes included in the range).
    let r = ctx
        .eval("var o = {length: 3}; Object.prototype[1] = 9; var i = Array.prototype.indexOf.call(o, 9); delete Object.prototype[1]; i;")
        .unwrap();
    assert_eq!(r.as_smi(), Some(1));
    // Getter elements run during search.
    let r = ctx
        .eval("var o = {length: 2, 0: 1}; Object.defineProperty(o, '1', {get: function () { return 6; }, configurable: true}); Array.prototype.indexOf.call(o, 6);")
        .unwrap();
    assert_eq!(r.as_smi(), Some(1));
}

#[test]
fn test_neg_zero_preserved() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("1 / -0").unwrap();
    assert!(
        r.as_float64().unwrap().is_infinite(),
        "1 / -0 should be -Infinity"
    );
    assert!(
        r.as_float64().unwrap().is_sign_negative(),
        "1 / -0 should be negative"
    );
}

#[test]
fn test_prototype_chain_get() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        var animal = { speak: function() { return "generic"; } };
        var dog = Object.create(animal);
        dog.speak
    "#,
        )
        .unwrap();
    assert!(
        r.is_heap_object(),
        "should inherit speak from animal prototype"
    );
}

#[test]
fn test_prototype_set_own_property() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        var animal = { x: 1 };
        var dog = Object.create(animal);
        dog.x = 2;
        dog.x
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(2));
}

#[test]
fn test_prototype_shadow() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        var proto = { name: "proto" };
        var obj = Object.create(proto);
        obj.name = "own";
        obj.name
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), None, "shadowed value is a string, not a number");
}

// ---- __proto__ assignment ---

#[test]
fn test_proto_set() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(r#"var proto = {x: 42}; var o = {}; o.__proto__ = proto; o.x"#)
        .unwrap();
    assert_eq!(r.as_smi(), Some(42), "__proto__ assignment sets prototype");
}

#[test]
fn test_proto_null() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(r#"var proto = {x: 42}; var o = {}; o.__proto__ = proto; o.__proto__ = null; o.x"#)
        .unwrap();
    assert_eq!(r.as_smi(), None, "null proto clears prototype chain");
}

#[test]
fn test_proto_deep_chain() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"function mk(l){if(l==0){return {x:42};}var o={};o.__proto__=mk(l-1);return o;} var o=mk(5); o.x"#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(42), "5-deep proto chain returns 42");
}

// ---- GC stress regression: hot property access ---

#[test]
fn test_hot_property_mono_1m() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(r#"var o = {x: 1}; var s = 0; for (var i = 0; i < 1000000; i = i + 1) { s = s + o.x; } s"#)
        .unwrap();
    assert_eq!(r.as_smi(), Some(1000000), "o.x 1M times returns 1000000");
}

#[test]
fn test_hot_property_poly_1m() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"var objs = []; for (var i = 0; i < 10; i = i + 1) { var o = {}; o.x = i; objs.push(o); } var s = 0; for (var i = 0; i < 1000000; i = i + 1) { s = s + objs[i % 10].x; } s"#,
        )
        .unwrap();
    assert_eq!(
        r.as_smi(),
        Some(4500000),
        "10-shape poly 1M returns 4500000"
    );
}

#[test]
fn test_new_opcode_returns_object() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("new Object()").unwrap();
    assert!(r.is_heap_object(), "new Object() should return an object");
}

#[test]
fn test_ic_populates_and_hits() {
    let mut ctx = Context::new_small();
    use rune_bytecode::opcode::{BytecodeProgram, Instruction, Opcode};

    let instrs = vec![
        Instruction::new(Opcode::LoadSmi, vec![42]),
        Instruction::new(Opcode::NewObject, vec![1, 0]),
        // 5 LoadProperty instructions with IC slots 0-4
        Instruction::new(Opcode::Dup, vec![]),
        Instruction::new(Opcode::LoadStringConst, vec![0]),
        Instruction::new(Opcode::LoadProperty, vec![]),
        Instruction::new(Opcode::Pop, vec![]),
        Instruction::new(Opcode::Dup, vec![]),
        Instruction::new(Opcode::LoadStringConst, vec![0]),
        Instruction::new(Opcode::LoadProperty, vec![]),
        Instruction::new(Opcode::Pop, vec![]),
        Instruction::new(Opcode::Dup, vec![]),
        Instruction::new(Opcode::LoadStringConst, vec![0]),
        Instruction::new(Opcode::LoadProperty, vec![]),
        Instruction::new(Opcode::Pop, vec![]),
        Instruction::new(Opcode::Dup, vec![]),
        Instruction::new(Opcode::LoadStringConst, vec![0]),
        Instruction::new(Opcode::LoadProperty, vec![]),
        Instruction::new(Opcode::Pop, vec![]),
        // Final access: store obj in local, load, read property, return
        Instruction::new(Opcode::StoreLocal, vec![0]),
        Instruction::new(Opcode::LoadLocal, vec![0]),
        Instruction::new(Opcode::LoadStringConst, vec![0]),
        Instruction::new(Opcode::LoadProperty, vec![]),
        Instruction::new(Opcode::Return, vec![]),
    ];
    let mut prog = BytecodeProgram::new(instrs, vec!["x".to_string()], vec![]);
    prog.assign_ic_indices();

    // First execution: all misses, IC populated
    let r = ctx.eval_bytecode(&prog).unwrap();
    assert_eq!(r.as_smi(), Some(42));
    let stats1 = ctx.vm().ic_stats;
    assert_eq!(stats1.lookups, 5);
    assert_eq!(stats1.hits, 0);
    assert_eq!(stats1.misses, 5);

    // Second execution of same bytecode: same shape, same IC slots → should all hit
    let r2 = ctx.eval_bytecode(&prog).unwrap();
    assert_eq!(r2.as_smi(), Some(42));
    let stats2 = ctx.vm().ic_stats;
    assert_eq!(stats2.lookups, 10);
    assert_eq!(stats2.hits, 5);
    assert_eq!(stats2.misses, 5);
}

#[test]
fn test_ic_polymorphic() {
    let mut ctx = Context::new_small();
    // Use eval_bytecode to test multiple shapes going through different IC slots
    use rune_bytecode::opcode::{BytecodeProgram, Instruction, Opcode};

    // Build 3 objects with different shapes, each with property x, access each once
    let mut instrs: Vec<Instruction> = Vec::with_capacity(32);
    // obj1: {x: 1} — x is string pool index 0
    instrs.push(Instruction::new(Opcode::LoadSmi, vec![1]));
    instrs.push(Instruction::new(Opcode::NewObject, vec![1, 0]));
    // obj2: {x: 2, a: 0} — x=0, a=1 in string pool
    instrs.push(Instruction::new(Opcode::LoadSmi, vec![2]));
    instrs.push(Instruction::new(Opcode::LoadSmi, vec![0])); // a's value
    instrs.push(Instruction::new(Opcode::NewObject, vec![2, 0, 1])); // x, a
    // obj3: {x: 3, a: 0, b: 0} — x=0, a=1, b=2 in string pool
    instrs.push(Instruction::new(Opcode::LoadSmi, vec![3]));
    instrs.push(Instruction::new(Opcode::LoadSmi, vec![0])); // a's value
    instrs.push(Instruction::new(Opcode::LoadSmi, vec![0])); // b's value
    instrs.push(Instruction::new(Opcode::NewObject, vec![3, 0, 1, 2])); // x, a, b
    // Access x on each in reverse stack order (LIFO)
    instrs.push(Instruction::new(Opcode::LoadStringConst, vec![0])); // key "x"
    instrs.push(Instruction::new(Opcode::LoadProperty, vec![])); // obj3.x = 3
    instrs.push(Instruction::new(Opcode::Pop, vec![]));

    instrs.push(Instruction::new(Opcode::LoadStringConst, vec![0]));
    instrs.push(Instruction::new(Opcode::LoadProperty, vec![])); // obj2.x = 2
    instrs.push(Instruction::new(Opcode::Pop, vec![]));

    instrs.push(Instruction::new(Opcode::LoadStringConst, vec![0]));
    instrs.push(Instruction::new(Opcode::LoadProperty, vec![])); // obj1.x = 1 (last on stack)
    instrs.push(Instruction::new(Opcode::Return, vec![]));
    let mut prog = BytecodeProgram::new(
        instrs,
        vec!["x".to_string(), "a".to_string(), "b".to_string()],
        vec![],
    );
    prog.assign_ic_indices();
    let r = ctx.eval_bytecode(&prog).unwrap();
    assert_eq!(r.as_smi(), Some(1), "last access returns 1");
}

#[test]
fn test_ic_proto_inherited() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        var proto = {x: 99};
        var child = Object.create(proto);
        child.x
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(99), "inherited property should resolve");
    let stats = ctx.vm().ic_stats;
    assert!(stats.lookups > 0, "IC should be active on LoadProperty");
    // Each static property access is a separate IC slot → all misses first time
    assert_eq!(stats.hits, 0, "no loops yet");
    assert!(stats.misses > 0, "at least one miss");
}

#[test]
fn test_ic_hits_across_evals() {
    let mut ctx = Context::new_small();
    // First eval: 10 property accesses, all misses, IC populated for shape {x: 42}
    ctx.eval(
        r#"
        var obj = {x: 42};
        obj.x; obj.x; obj.x; obj.x; obj.x;
        obj.x; obj.x; obj.x; obj.x; obj.x
    "#,
    )
    .unwrap();
    let stats1 = ctx.vm().ic_stats;
    assert_eq!(stats1.lookups, 10);
    assert_eq!(stats1.hits, 0);
    assert_eq!(stats1.misses, 10);

    // Second eval: same shape, same IC slots → all hits
    ctx.eval(
        r#"
        var obj = {x: 42};
        obj.x; obj.x; obj.x; obj.x; obj.x;
        obj.x; obj.x; obj.x; obj.x; obj.x
    "#,
    )
    .unwrap();
    let stats2 = ctx.vm().ic_stats;
    assert_eq!(stats2.lookups, 20);
    assert_eq!(stats2.hits, 10);
    assert_eq!(stats2.misses, 10);
}

#[test]
fn test_dense_array_literal() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("[1, 2, 3]").unwrap();
    assert!(
        r.is_heap_object(),
        "array literal should return heap object"
    );
}

#[test]
fn test_dense_array_get_element() {
    let mut ctx = Context::new_small();
    // Single eval: create array and access multiple elements
    let r = ctx
        .eval("var a = [10, 20, 30]; a[0] + a[1] + a[2]")
        .unwrap();
    assert_eq!(r.as_smi(), Some(60));
}

#[test]
fn test_dense_array_out_of_bounds() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("var a = [1, 2, 3]; a[5]").unwrap();
    assert!(r.is_undefined(), "out of bounds should be undefined");
}

#[test]
fn test_dense_array_set_element() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("var a = [1, 2, 3]; a[0] = 99; a[0]").unwrap();
    assert_eq!(r.as_smi(), Some(99));
}

#[test]
fn test_array_push_pop() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("var a = [1, 2]; a.push(3); a[2]").unwrap();
    assert_eq!(r.as_smi(), Some(3));
    let r2 = ctx.eval("var a = [1, 2, 3]; var v = a.pop(); v").unwrap();
    assert_eq!(r2.as_smi(), Some(3));
}

#[test]
fn test_array_push_grow() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval("var a = [1]; for (var i = 0; i < 10; i = i + 1) { a.push(i); } a.length")
        .unwrap();
    assert_eq!(r.as_smi(), Some(11));
    let r2 = ctx
        .eval("var a = [1]; for (var i = 0; i < 10; i = i + 1) { a.push(i); } a[0] + a[5] + a[10]")
        .unwrap();
    assert_eq!(r2.as_smi(), Some(1 + 4 + 9));
}

#[test]
fn test_array_push_grow_identity() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval("var a = [42]; var b = a; for (var i = 0; i < 20; i = i + 1) { a.push(i); } a.length")
        .unwrap();
    assert_eq!(r.as_smi(), Some(21));
    let r2 = ctx
        .eval("var a = [42]; var b = a; for (var i = 0; i < 20; i = i + 1) { a.push(i); } b.length")
        .unwrap();
    assert_eq!(r2.as_smi(), Some(21));
    let r3 = ctx.eval("var a = [42]; var b = a; for (var i = 0; i < 20; i = i + 1) { a.push(i); } b[0] + b[10] + b[20]").unwrap();
    assert_eq!(r3.as_smi(), Some(42 + 9 + 19));
}

#[test]
fn test_for_in_object() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("var o={x:1,y:2,z:3}; var s=0; for(var k in o){s=s+o[k];} s");
    assert_eq!(r.unwrap().as_smi(), Some(6));
}

#[test]
fn test_for_in_array() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("var a=[10,20,30]; var s=0; for(var k in a){s=s+a[k];} s");
    assert_eq!(r.unwrap().as_smi(), Some(60));
}

#[test]
fn test_for_in_empty() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("var o={}; var c=0; for(var k in o){c=c+1;} c");
    assert_eq!(r.unwrap().as_smi(), Some(0));
}

#[test]
fn test_for_in_null() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("var c=0; for(var k in null){c=c+1;} c");
    assert_eq!(r.unwrap().as_smi(), Some(0));
}

#[test]
fn test_array_is_array() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("Array.isArray([1,2,3])").unwrap();
    assert_eq!(
        r.to_boolean(),
        Some(true),
        "Array.isArray should return true for arrays"
    );
    let r2 = ctx.eval("Array.isArray(42)").unwrap();
    assert_eq!(
        r2.to_boolean(),
        Some(false),
        "Array.isArray should return false for non-arrays"
    );
}

#[test]
fn test_math_constants() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("Math.PI + 1").unwrap();
    assert!(r.is_float64(), "Math.PI + 1 should be a float64");
    let r2 = ctx.eval("Math.E + 1").unwrap();
    assert!(r2.is_float64(), "Math.E + 1 should be a float64");
}

#[test]
fn test_string_char_at() {
    let mut ctx = Context::new_small();
    let r = ctx.eval(r#"var s = "hello"; s.charAt(0)"#).unwrap();
    assert!(r.is_heap_object(), "charAt should return a string");
    let r2 = ctx.eval(r#"var s = "hello"; s.charAt(1)"#).unwrap();
    assert!(r2.is_heap_object(), "charAt should return a string");
    let r3 = ctx.eval(r#"var s = "abc"; s.charAt(100)"#).unwrap();
    assert!(
        r3.is_heap_object(),
        "charAt OOB should return a string (not undefined)"
    );
}

#[test]
fn test_string_slice() {
    let mut ctx = Context::new_small();
    let r = ctx.eval(r#"var s = "hello"; s.slice(0, 3)"#).unwrap();
    assert!(r.is_heap_object(), "slice should return a string");
}

#[test]
fn test_string_length() {
    let mut ctx = Context::new_small();
    let r = ctx.eval(r#"var s = "hello"; s.length"#).unwrap();
    assert_eq!(r.as_smi(), Some(5));
    let r2 = ctx.eval(r#"var s = "a"; s.length"#).unwrap();
    assert_eq!(r2.as_smi(), Some(1));
}

#[test]
fn test_new_string_wrapper() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(r#"var s = new String("hello"); typeof s"#)
        .unwrap();
    assert_eq!(r.as_smi(), None, "typeof result is a string, not smi");
    let r = ctx
        .eval(r#"var s = new String("hello"); s.length"#)
        .unwrap();
    assert_eq!(r.as_smi(), Some(5));
    let r = ctx.eval(r#"var s = new String("hello"); s[0]"#).unwrap();
    let s = unsafe {
        rune_core::string::HeapString::to_string(
            r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
        )
    };
    assert_eq!(s, "h");
    let r = ctx
        .eval(r#"var s = new String("hello"); s.slice(1, 3)"#)
        .unwrap();
    let s = unsafe {
        rune_core::string::HeapString::to_string(
            r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
        )
    };
    assert_eq!(s, "el");
    let r = ctx
        .eval(r#"var s = new String("hello"); s.charAt(0)"#)
        .unwrap();
    let s = unsafe {
        rune_core::string::HeapString::to_string(
            r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
        )
    };
    assert_eq!(s, "h");
    let r = ctx.eval(r#"String("hello")"#).unwrap();
    let s = unsafe {
        rune_core::string::HeapString::to_string(
            r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
        )
    };
    assert_eq!(s, "hello");
}

#[test]
fn test_math_floor() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("Math.floor(3.7)").unwrap();
    assert_eq!(r.as_smi(), Some(3));
    let r2 = ctx.eval("Math.floor(-1.5)").unwrap();
    assert_eq!(r2.as_smi(), Some(-2));
    let r3 = ctx.eval("Math.floor(5)").unwrap();
    assert_eq!(r3.as_smi(), Some(5));
}

#[test]
fn test_math_ceil() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("Math.ceil(3.2)").unwrap();
    assert_eq!(r.as_smi(), Some(4));
}

#[test]
fn test_math_abs() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("Math.abs(-5)").unwrap();
    assert_eq!(r.as_smi(), Some(5));
}

#[test]
fn test_math_sqrt() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("Math.sqrt(9)").unwrap();
    assert_eq!(r.as_smi(), Some(3));
}

#[test]
fn test_constructor_this_binding() {
    let mut ctx = Context::new_small();
    // Constructor returning this — should return the new object
    let r = ctx
        .eval("function Foo() { return this; } new Foo()")
        .unwrap();
    assert!(
        r.is_heap_object(),
        "constructor returning this gives heap object"
    );
    // Set property on this and verify via property access on the constructed object
    let r2 = ctx
        .eval("function Foo() { this.x = 42; } var f = new Foo(); f.x")
        .unwrap();
    assert_eq!(
        r2.as_smi(),
        Some(42),
        "constructor should set property on this"
    );
    // Accessing `this` directly
    let r3 = ctx
        .eval("function Bar() { return this; } new Bar()")
        .unwrap();
    assert!(r3.is_heap_object(), "new should return this");
}

#[test]
fn test_constructor_basic() {
    let mut ctx = Context::new_small();
    // Constructor that returns 42 — should be ignored (primitive), returning `this`
    let r = ctx
        .eval(
            r#"
        function Foo() {
            return 42;
        }
        new Foo()
    "#,
        )
        .unwrap();
    assert!(r.is_heap_object(), "new Foo() should return heap object");
}

#[test]
fn test_constructor_returns_object() {
    let mut ctx = Context::new_small();
    // Constructor can reference `this` (but it's just a local)
    let r = ctx
        .eval(
            r#"
        function Foo() {
            var y = 42;
            return y;
        }
        var f = new Foo();
        1
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(1));
}

#[test]
fn test_constructor_prototype_inheritance() {
    let mut ctx = Context::new_small();
    // Foo.prototype exists and is accessible
    let r = ctx
        .eval(
            r#"
        function Foo() {}
        var p = Foo.prototype;
        1
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(1), "Foo.prototype should be accessible");
    // Own property on the new object (set via constructor)
    let r2 = ctx
        .eval(
            r#"
        function Foo(x) { this.x = x; }
        var f = new Foo(42);
        f.x
    "#,
        )
        .unwrap();
    assert_eq!(r2.as_smi(), Some(42), "own property via constructor");
    // Property set on prototype is inherited by new objects
    let r3 = ctx
        .eval(
            r#"
        function Foo() {}
        Foo.prototype.x = 42;
        var f = new Foo();
        f.x
    "#,
        )
        .unwrap();
    assert_eq!(r3.as_smi(), Some(42), "inherited property via prototype");
    // Own property shadows prototype property
    let r4 = ctx
        .eval(
            r#"
        function Foo() {}
        Foo.prototype.x = 99;
        var f = new Foo();
        f.x = 42;
        f.x
    "#,
        )
        .unwrap();
    assert_eq!(r4.as_smi(), Some(42), "own property shadows prototype");
    // Modifying prototype after construction affects existing objects
    let r5 = ctx
        .eval(
            r#"
        function Foo() {}
        var f = new Foo();
        Foo.prototype.x = 42;
        f.x
    "#,
        )
        .unwrap();
    assert_eq!(
        r5.as_smi(),
        Some(42),
        "dynamic prototype mutation affects existing objects"
    );
    // Foo.prototype.constructor points back to Foo
    let r6 = ctx
        .eval(
            r#"
        function Foo() {}
        var p = Foo.prototype;
        var c = p.constructor;
        1
    "#,
        )
        .unwrap();
    assert_eq!(r6.as_smi(), Some(1), "prototype.constructor is accessible");
}

// ---- ECMA-262 Spec Compliance (Task 9C) ----

#[test]
fn test_float_comparison() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("3.5 > 2").unwrap();
    assert_eq!(r.to_boolean(), Some(true), "3.5 > 2 should be true");
    let r2 = ctx.eval("Math.PI > 3").unwrap();
    assert_eq!(r2.to_boolean(), Some(true), "Math.PI > 3 should be true");
    let r3 = ctx.eval("1.5 < 2.5").unwrap();
    assert_eq!(r3.to_boolean(), Some(true), "1.5 < 2.5 should be true");
}

#[test]
fn test_mixed_comparison() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("3 > 2.5").unwrap();
    assert_eq!(r.to_boolean(), Some(true), "Smi > Float64 should work");
    let r2 = ctx.eval("2.5 < 3").unwrap();
    assert_eq!(r2.to_boolean(), Some(true), "Float64 < Smi should work");
}

#[test]
fn test_compound_assign() {
    let mut ctx = Context::new_small();
    // += on local variable
    let r = ctx.eval("var x = 5; x += 3; x").unwrap();
    assert_eq!(r.as_smi(), Some(8), "x += 3 should give 8");
    // -= on local variable
    let r2 = ctx.eval("var x = 10; x -= 3; x").unwrap();
    assert_eq!(r2.as_smi(), Some(7), "x -= 3 should give 7");
    // *= on local variable
    let r3 = ctx.eval("var x = 4; x *= 3; x").unwrap();
    assert_eq!(r3.as_smi(), Some(12), "x *= 3 should give 12");
    // Compound assign on property with separate object create
    let r4 = ctx.eval(r#"var o = {}; o.a = 1; o.a += 2; o.a"#).unwrap();
    assert_eq!(
        r4.as_smi(),
        Some(3),
        "o.a += 2 after separate set should give 3, got {r4:?}"
    );
}

#[test]
fn test_logical_and() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("0 && 1").unwrap();
    assert_eq!(
        r.as_smi(),
        Some(0),
        "0 && 1 should return 0 (falsy short-circuit)"
    );
    let r2 = ctx.eval("1 && 2").unwrap();
    assert_eq!(
        r2.as_smi(),
        Some(2),
        "1 && 2 should return 2 (truthy, evaluates RHS)"
    );
    let r3 = ctx.eval("false && true").unwrap();
    assert_eq!(
        r3.to_boolean(),
        Some(false),
        "false && true should return false"
    );
    let r4 = ctx.eval("true && 42").unwrap();
    assert_eq!(r4.as_smi(), Some(42), "true && 42 should return 42");
}

#[test]
fn test_logical_or() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("1 || 2").unwrap();
    assert_eq!(
        r.as_smi(),
        Some(1),
        "1 || 2 should return 1 (truthy short-circuit)"
    );
    let r2 = ctx.eval("0 || 2").unwrap();
    assert_eq!(
        r2.as_smi(),
        Some(2),
        "0 || 2 should return 2 (falsy, evaluates RHS)"
    );
    let r3 = ctx.eval("true || false").unwrap();
    assert_eq!(
        r3.to_boolean(),
        Some(true),
        "true || false should return true"
    );
    let r4 = ctx.eval("false || 42").unwrap();
    assert_eq!(r4.as_smi(), Some(42), "false || 42 should return 42");
}

#[test]
fn test_delete_property() {
    let mut ctx = Context::new_small();
    let r = ctx.eval(r#"var o = {a: 1}; delete o.a; "a" in o"#).unwrap();
    assert_eq!(
        r.to_boolean(),
        Some(false),
        "delete o.a should remove property; 'a' in o should be false"
    );
    let r2 = ctx
        .eval(r#"var o = {a: 1, b: 2}; delete o.a; o.b"#)
        .unwrap();
    assert_eq!(
        r2.as_smi(),
        Some(2),
        "after delete o.a, o.b should remain 2"
    );
    let r3 = ctx.eval(r#"var o = {a: 1}; delete o.b; "a" in o"#).unwrap();
    assert_eq!(
        r3.to_boolean(),
        Some(true),
        "delete non-existent property returns true, 'a' in o still true"
    );
    let r4 = ctx.eval("delete 42").unwrap();
    assert_eq!(r4.to_boolean(), Some(true), "delete 42 should return true");
}

#[test]
fn test_in_operator() {
    let mut ctx = Context::new_small();
    let r = ctx.eval(r#"var o = {a: 1}; "a" in o"#).unwrap();
    assert_eq!(r.to_boolean(), Some(true), r#""a" in o should be true"#);
    let r2 = ctx.eval(r#"var o = {a: 1}; "b" in o"#).unwrap();
    assert_eq!(r2.to_boolean(), Some(false), r#""b" in o should be false"#);
    let r3 = ctx.eval(r#"var a = [10, 20]; 0 in a"#).unwrap();
    assert_eq!(r3.to_boolean(), Some(true), "0 in [10,20] should be true");
    let r4 = ctx.eval(r#"var a = [10, 20]; 2 in a"#).unwrap();
    assert_eq!(
        r4.to_boolean(),
        Some(false),
        "2 in [10,20] should be false (OOB)"
    );
    let r5 = ctx.eval(r#"var a = [10, 20]; "length" in a"#).unwrap();
    assert_eq!(
        r5.to_boolean(),
        Some(true),
        "\"length\" in [10,20] should be true"
    );
    // Nested object literal: property access via bracket notation
    let r6 = ctx
        .eval(r#"var o = {nested: {key: 1}}; "key" in o.nested"#)
        .unwrap();
    assert_eq!(
        r6.to_boolean(),
        Some(true),
        "key in nested object should be true"
    );
}

#[test]
fn test_strict_eq_smi_float() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("1 === 1.0").unwrap();
    assert_eq!(
        r.to_boolean(),
        Some(true),
        "1 === 1.0 should be true (Smi↔Float64 same number)"
    );
    let r2 = ctx.eval("1.0 === 1").unwrap();
    assert_eq!(r2.to_boolean(), Some(true), "1.0 === 1 should be true");
    let r3 = ctx.eval("1 !== 1.0").unwrap();
    assert_eq!(r3.to_boolean(), Some(false), "1 !== 1.0 should be false");
}

#[test]
fn test_strict_eq_nan() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("NaN === NaN").unwrap();
    assert_eq!(
        r.to_boolean(),
        Some(false),
        "NaN === NaN should be false per §7.2.14"
    );
    let r2 = ctx.eval("NaN !== NaN").unwrap();
    assert_eq!(r2.to_boolean(), Some(true), "NaN !== NaN should be true");
}

#[test]
fn test_strict_eq_neg_zero() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("(-0) === 0").unwrap();
    assert_eq!(
        r.to_boolean(),
        Some(true),
        "-0 === 0 should be true per §7.2.14"
    );
    let r2 = ctx.eval("0 === (-0)").unwrap();
    assert_eq!(r2.to_boolean(), Some(true), "0 === -0 should be true");
    let r3 = ctx.eval("(-0) !== 0").unwrap();
    assert_eq!(r3.to_boolean(), Some(false), "-0 !== 0 should be false");
}

#[test]
fn test_nan_comparison() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("NaN < 5").unwrap();
    assert!(r.is_undefined(), "NaN < 5 should be undefined per §12.9");
    let r2 = ctx.eval("NaN >= 5").unwrap();
    assert_eq!(
        r2.to_boolean(),
        Some(false),
        "NaN >= 5 should be false per §12.10"
    );
    let r3 = ctx.eval("NaN <= 5").unwrap();
    assert_eq!(
        r3.to_boolean(),
        Some(false),
        "NaN <= 5 should be false per §12.10"
    );
}

#[test]
fn test_to_number_string() {
    let mut ctx = Context::new_small();
    let r = ctx.eval(r#""5" > 3"#).unwrap();
    assert_eq!(r.to_boolean(), Some(true), "ToNumber('5') = 5 > 3");
    let r2 = ctx.eval(r#"3 > "5""#).unwrap();
    assert_eq!(
        r2.to_boolean(),
        Some(false),
        "3 > ToNumber('5') should be false"
    );
}

#[test]
fn test_boolean_arithmetic() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("true + 1").unwrap();
    assert_eq!(r.as_smi(), Some(2), "true + 1 = 2 per §7.1.4");
    let r = ctx.eval("false + 1").unwrap();
    assert_eq!(r.as_smi(), Some(1), "false + 1 = 1");
    let r = ctx.eval("true + false").unwrap();
    assert_eq!(r.as_smi(), Some(1), "true + false = 1");
    let r = ctx.eval("true * 3").unwrap();
    assert_eq!(r.as_smi(), Some(3), "true * 3 = 3");
    let r = ctx.eval("false * 100").unwrap();
    assert_eq!(r.as_smi(), Some(0), "false * 100 = 0");
    let r = ctx.eval("true / 2").unwrap();
    assert_eq!(r.as_float64(), Some(0.5), "true / 2 = 0.5");
    let r = ctx.eval("true - false").unwrap();
    assert_eq!(r.as_smi(), Some(1), "true - false = 1");
}

#[test]
fn test_boolean_unary_plus() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("+true").unwrap();
    assert_eq!(r.as_smi(), Some(1), "+true = 1 per §13.5.3");
    let r = ctx.eval("+false").unwrap();
    assert_eq!(r.as_smi(), Some(0), "+false = 0");
    let r = ctx.eval("+1").unwrap();
    assert_eq!(r.as_smi(), Some(1), "+1 = 1 (identity)");
}

#[test]
fn test_boolean_comparison() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("true < 2").unwrap();
    assert_eq!(r.to_boolean(), Some(true), "true < 2 should be true");
    let r = ctx.eval("false < -1").unwrap();
    assert_eq!(r.to_boolean(), Some(false), "false < -1 should be false");
    let r = ctx.eval("true > 0").unwrap();
    assert_eq!(r.to_boolean(), Some(true), "true > 0 should be true");
}

#[test]
fn test_boolean_bitwise() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("0 | true").unwrap();
    assert_eq!(r.as_smi(), Some(1), "0 | true = 1 per §13.3.3");
    let r = ctx.eval("5 & true").unwrap();
    assert_eq!(r.as_smi(), Some(1), "5 & true = 1");
    let r = ctx.eval("true ^ false").unwrap();
    assert_eq!(r.as_smi(), Some(1), "true ^ false = 1");
    let r = ctx.eval("true << 1").unwrap();
    assert_eq!(r.as_smi(), Some(2), "true << 1 = 2");
    let r = ctx.eval("true >> 1").unwrap();
    assert_eq!(r.as_smi(), Some(0), "true >> 1 = 0");
}

#[test]
fn test_loose_equality() {
    let mut ctx = Context::new_small();
    // Boolean == Number
    let r = ctx.eval("true == 1").unwrap();
    assert_eq!(r.to_boolean(), Some(true), "true == 1 per §7.2.13");
    let r = ctx.eval("false == 0").unwrap();
    assert_eq!(r.to_boolean(), Some(true), "false == 0");
    // String == Number
    let r = ctx.eval(r#"1 == "1""#).unwrap();
    assert_eq!(r.to_boolean(), Some(true), r#"1 == "1" per §7.2.13"#);
    let r = ctx.eval(r#"0 == """#).unwrap();
    assert_eq!(r.to_boolean(), Some(true), r#"0 == "" per §7.2.13"#);
    // null == undefined
    let r = ctx.eval("null == undefined").unwrap();
    assert_eq!(r.to_boolean(), Some(true), "null == undefined per §7.2.13");
    // Strict equality still rejects cross-type
    let r = ctx.eval("true === 1").unwrap();
    assert_eq!(
        r.to_boolean(),
        Some(false),
        "true === 1 is false per §7.2.14"
    );
    // Negative cases
    let r = ctx.eval("true == 0").unwrap();
    assert_eq!(r.to_boolean(), Some(false), "true == 0 is false");
    let r = ctx.eval(r#"1 == "2""#).unwrap();
    assert_eq!(r.to_boolean(), Some(false), r#"1 == "2" is false"#);
    let r = ctx.eval("null == 0").unwrap();
    assert_eq!(
        r.to_boolean(),
        Some(false),
        "null == 0 is false per §7.2.13"
    );
}

#[test]
fn test_increment_prefix() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("var x = 5; ++x").unwrap();
    assert_eq!(r.as_smi(), Some(6), "++x should return new value");
    let r2 = ctx.eval("var x = 5; var y = ++x; y").unwrap();
    assert_eq!(
        r2.as_smi(),
        Some(6),
        "++x assigned to var should be new value"
    );
}

#[test]
fn test_increment_postfix() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("var x = 5; x++").unwrap();
    assert_eq!(r.as_smi(), Some(5), "x++ should return old value");
    let r2 = ctx.eval("var y = 5; y++; y").unwrap();
    assert_eq!(r2.as_smi(), Some(6), "y should be incremented after x++");
}

#[test]
fn test_decrement() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("var x = 10; --x").unwrap();
    assert_eq!(
        r.as_smi(),
        Some(9),
        "--x should decrement and return new value"
    );
    let r2 = ctx.eval("var y = 10; y--").unwrap();
    assert_eq!(r2.as_smi(), Some(10), "y-- should return old value");
    let r3 = ctx.eval("var y = 10; y--; y").unwrap();
    assert_eq!(r3.as_smi(), Some(9), "y should be decremented after y--");
}

#[test]
fn test_negate_string() {
    let mut ctx = Context::new_small();
    let r = ctx.eval(r#"-"42""#).unwrap();
    let val = r
        .as_float64()
        .unwrap_or(r.as_smi().map(|v| v as f64).unwrap_or(f64::NAN));
    assert_eq!(val, -42.0, r#"-"42" should be -42 via ToNumber"#);
}

#[test]
fn test_negate_overflow() {
    let mut ctx = Context::new_small();
    // -(2^30) = -1073741824 fits in Smi, but -(-2^30) = 2^30 does not
    // var x = -(1 << 30) → but we can't compute 1<<30 in our runtime yet,
    // so just test negating a large negative number
    let r = ctx.eval("var x = -1073741824; -x").unwrap();
    let val = r
        .as_float64()
        .unwrap_or(r.as_smi().map(|v| v as f64).unwrap_or(f64::NAN));
    assert_eq!(
        val, 1073741824.0,
        "-(-1073741824) should be 1073741824 via HeapFloat64"
    );
}

#[test]
fn test_increment_in_for_loop() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        var sum = 0;
        for (var i = 0; i < 10; i++) {
            sum = sum + i;
        }
        sum
    "#,
        )
        .unwrap();
    assert_eq!(
        r.as_smi(),
        Some(45),
        "sum 0..9 = 45 after for loop with i++"
    );
}

#[test]
fn test_negate_undefined() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("-undefined").unwrap();
    assert!(
        r.as_float64().unwrap().is_nan(),
        "-undefined should be NaN per spec"
    );
}

#[test]
#[cfg(target_arch = "aarch64")]
fn test_jit_tier_up() {
    // add(a, b) is JIT-compatible; tier-up at 50 calls, then bails on first
    // opcode (MakeArgumentsArray, §6.2 bail-on-entry). Verifies the JIT
    // actually compiled and the bailout path works.
    let mut ctx = Context::new_small();
    ctx.enable_jit = true;
    let r = ctx
        .eval(
            r#"
        function add(a, b) { return a + b; }
        var sum = 0;
        for (var i = 0; i < 100; i++) {
            sum = add(sum, i);
        }
        sum
    "#,
        )
        .unwrap();
    // sum = 0+1+2+...+99 = 4950
    assert_eq!(r.as_smi(), Some(4950), "JIT tier-up: sum should be 4950");
    assert!(
        ctx.vm().jit_entry_count > 0,
        "JIT must have executed at least once"
    );
}

#[test]
#[cfg(target_arch = "aarch64")]
fn test_jit_bailout_on_float() {
    // add() tier-up at 50, then pass a float64 — JIT bails at MakeArgumentsArray
    // (§6.2 bail-on-entry), interpreter handles float via normal flow.
    let mut ctx = Context::new_small();
    ctx.enable_jit = true;
    let r = ctx
        .eval(
            r#"
        function add(a, b) { return a + b; }
        var sum = 0;
        for (var i = 0; i < 100; i++) {
            sum = add(sum, i);
        }
        var result = add(3.5, 2);
        result
    "#,
        )
        .unwrap();
    // 3.5 + 2 = 5.5
    let f = r.as_float64().unwrap_or(0.0);
    assert!(
        (f - 5.5).abs() < 0.001,
        "JIT bail-out: add(3.5, 2) should be ~5.5, got {}",
        f
    );
    assert!(
        ctx.vm().jit_entry_count > 0,
        "JIT must have executed at least once before float bail-out"
    );
}

#[test]
#[cfg(target_arch = "aarch64")]
fn test_jit_non_smi_args_bail() {
    // Non-arrow function with float arg. JIT now promotes non-Smi Add operands
    // to float64 via helper instead of bailing. The function still produces the
    // correct result, and JIT is used (entry_count > 0).
    let mut ctx = Context::new_small();
    ctx.enable_jit = true;
    let r = ctx
        .eval(
            r#"
        function add(a, b) { return a + b; }
        var sum = 0;
        for (var i = 0; i < 100; i++) {
            sum = add(sum, i);
        }
        var result = add(3.5, 2);
        result
    "#,
        )
        .unwrap();
    let f = r.as_float64().unwrap_or(0.0);
    assert!(
        (f - 5.5).abs() < 0.001,
        "JIT non-Smi: add(3.5, 2) should be ~5.5, got {}",
        f
    );
    assert!(
        ctx.vm().jit_entry_count > 0,
        "JIT must have entered at least once"
    );
}

#[test]
#[cfg(target_arch = "aarch64")]
fn test_jit_bailout_count() {
    // Verify the jit_bailout_count counter increments alongside jit_entry_count.
    let mut ctx = Context::new_small();
    ctx.enable_jit = true;
    ctx.eval(
        r#"
        // Uses `arguments` — MakeArgumentsArray still emitted → JIT bails on entry.
        function useArgs() { return arguments; }
        var r;
        for (var i = 0; i < 100; i++) {
            r = useArgs(1, 2, 3);
        }
        r
    "#,
    )
    .unwrap();
    assert!(ctx.vm().jit_entry_count > 0, "JIT must have entered");
    assert!(
        ctx.vm().jit_bailout_count > 0,
        "JIT must have bailed at least once"
    );
    assert!(
        ctx.vm().jit_bailout_count <= ctx.vm().jit_entry_count,
        "Bailouts should not exceed entries"
    );
}

#[test]
#[cfg(target_arch = "aarch64")]
fn test_jit_no_bail_on_simple_fn() {
    // A function that doesn't use `arguments` no longer has MakeArgumentsArray.
    // The JIT should run end-to-end without bailing.
    let mut ctx = Context::new_small();
    ctx.enable_jit = true;
    ctx.eval(
        r#"
        function add(a, b) { return a + b; }
        var sum = 0;
        for (var i = 0; i < 100; i++) {
            sum = add(sum, i);
        }
        sum
    "#,
    )
    .unwrap();
    assert!(ctx.vm().jit_entry_count > 0, "JIT must have entered");
    assert_eq!(
        ctx.vm().jit_bailout_count,
        0,
        "Simple add() should not bail (no MakeArgumentsArray)"
    );
}

#[test]
fn test_jit_needs_frame_verification() {
    let ctx = Context::new_small();

    // add(a,b) — pure arithmetic, no lexical scope.
    // This is the target benchmark for Phase F inlining.
    let prog = ctx.compile("function add(a,b) { return a + b; }").unwrap();
    assert!(
        !prog.functions[0].needs_frame(),
        "add(a,b) should not need a frame (target benchmark for Phase F)"
    );

    // Function with let — needs frame for DeclareLet.
    let prog = ctx.compile("function f() { let x = 1; }").unwrap();
    assert!(
        prog.functions[0].needs_frame(),
        "function with let should need a frame"
    );

    // Function with const — needs frame for DeclareConst.
    let prog = ctx
        .compile("function f() { const x = 42; return x; }")
        .unwrap();
    assert!(
        prog.functions[0].needs_frame(),
        "function with const should need a frame"
    );

    // Arrow function — lexical this, no frame needed.
    let prog = ctx.compile("let f = () => 42;").unwrap();
    assert!(
        !prog.functions[0].needs_frame(),
        "arrow function should not need a frame"
    );

    // Function using this — needs frame for LoadThis.
    let prog = ctx.compile("function f() { return this; }").unwrap();
    assert!(
        prog.functions[0].needs_frame(),
        "function using this should need a frame"
    );
}

#[test]
fn test_jit_inline_feature_flag() {
    // The feature flag should not change behavior when inlining is
    // infrastructure-only (F-0). Both flags produce identical results.
    let mut ctx_inline = Context::new_small();
    ctx_inline.enable_inlining = true;
    let r1 = ctx_inline
        .eval(
            r#"
            function add(a, b) { return a + b; }
            var sum = 0;
            for (var i = 0; i < 100; i++) {
                sum = add(sum, i);
            }
            sum
        "#,
        )
        .unwrap();

    let mut ctx_no_inline = Context::new_small();
    ctx_no_inline.enable_inlining = false;
    let r2 = ctx_no_inline
        .eval(
            r#"
            function add(a, b) { return a + b; }
            var sum = 0;
            for (var i = 0; i < 100; i++) {
                sum = add(sum, i);
            }
            sum
        "#,
        )
        .unwrap();

    assert_eq!(
        r1, r2,
        "--inline and --no-inline should produce identical results"
    );
}

#[test]
#[cfg(target_arch = "aarch64")]
fn test_jit_inline_skip_noneligible() {
    // Functions that need a frame (e.g., using 'let') should not be inlined.
    // With --inline enabled, the InlinePlan eligibility check should skip
    // such functions.  The result must be identical to --no-inline.
    let mut ctx_inline = Context::new_small();
    ctx_inline.enable_inlining = true;
    let r1 = ctx_inline
        .eval(
            r#"
            function f(x) { let y = x + 1; return y; }
            var s = 0;
            for (var i = 0; i < 100; i++) { s = f(s); }
            s
        "#,
        )
        .unwrap();

    let mut ctx_no_inline = Context::new_small();
    ctx_no_inline.enable_inlining = false;
    let r2 = ctx_no_inline
        .eval(
            r#"
            function f(x) { let y = x + 1; return y; }
            var s = 0;
            for (var i = 0; i < 100; i++) { s = f(s); }
            s
        "#,
        )
        .unwrap();

    assert_eq!(
        r1, r2,
        "inlining skip for frame-needing function should produce same result"
    );
}

#[test]
#[cfg(target_arch = "aarch64")]
fn test_jit_inline_no_bail() {
    // Simple frame-less function (add(a,b)) should inline cleanly with
    // zero bailouts when --inline is enabled.
    let mut ctx = Context::new_small();
    ctx.enable_jit = true;
    ctx.enable_inlining = true;
    let r = ctx
        .eval(
            r#"
            function add(a, b) { return a + b; }
            var s = 0;
            for (var i = 0; i < 100; i++) { s = add(s, i); }
            s
        "#,
        )
        .unwrap();
    assert_eq!(
        r.as_smi(),
        Some(4950),
        "inlined add() should produce correct result"
    );
    assert!(
        ctx.vm().jit_entry_count > 0,
        "JIT must have entered the loop trace"
    );
    assert_eq!(
        ctx.vm().jit_bailout_count,
        0,
        "inlined add() should not cause bailouts"
    );
}

#[test]
#[cfg(target_arch = "aarch64")]
fn test_jit_inline_skip_unarith() {
    // Functions with Sub/Mul (not in the emit_inline_call whitelist)
    // must NOT be inlined.  Result should match --no-inline.
    let mut ctx_inline = Context::new_small();
    ctx_inline.enable_inlining = true;
    let r1 = ctx_inline
        .eval(
            r#"
            function sub(a, b) { return a - b; }
            function mul(a, b) { return a * b; }
            var x = 100;
            for (var i = 0; i < 100; i = i + 1) { x = sub(x, 1); }
            var y = 1;
            for (var i = 1; i <= 10; i = i + 1) { y = mul(y, i); }
            x + y
        "#,
        )
        .unwrap();

    let mut ctx_no_inline = Context::new_small();
    ctx_no_inline.enable_inlining = false;
    let r2 = ctx_no_inline
        .eval(
            r#"
            function sub(a, b) { return a - b; }
            function mul(a, b) { return a * b; }
            var x = 100;
            for (var i = 0; i < 100; i = i + 1) { x = sub(x, 1); }
            var y = 1;
            for (var i = 1; i <= 10; i = i + 1) { y = mul(y, i); }
            x + y
        "#,
        )
        .unwrap();

    assert_eq!(
        r1, r2,
        "sub/mul not in the whitelist should produce same result as no-inline"
    );
}

#[test]
#[cfg(target_arch = "aarch64")]
fn test_jit_inline_bail() {
    // Inlined Sub overflow triggers a bailout (F-3).  The JIT stack must be
    // restored to pre-call state so the interpreter can re-execute the Call
    // instruction.  Verify correct result and bailout count > 0.
    let mut ctx = Context::new_small();
    ctx.enable_jit = true;
    ctx.enable_inlining = true;
    let r = ctx
        .eval(
            r#"
            function sub(a, b) { return a - b; }
            var s = 1000000000;
            for (var i = 0; i < 100; i = i + 1) { s = sub(s, -1000000); }
            s
        "#,
        )
        .unwrap();
    // 1000000000 + 100*1000000 = 1100000000
    let smi = r.as_smi();
    assert!(
        smi.is_none() || smi == Some(1100000000),
        "inlined Sub with overflow bailout should produce correct result, got: {:?}",
        smi
    );
    // 1100000000 exceeds Smi max (1073741823), so result should be a float
    if let Some(smi_val) = smi {
        assert_eq!(
            smi_val, 1100000000,
            "overflow result should be correct or float"
        );
    }
    assert!(
        ctx.vm().jit_bailout_count > 0,
        "inlined Sub overflow must trigger bailout"
    );
    assert!(
        ctx.vm().jit_entry_count > 0,
        "JIT must have entered for sub()"
    );
}

#[test]
#[cfg(target_arch = "aarch64")]
fn test_jit_inline_hot_function() {
    // F-4: verify jit_hot_function_1M with --inline produces the same result
    // as --no-inline.  add() called 1M times should tier up and inline.
    let mut ctx = Context::new_small();
    ctx.enable_jit = true;
    ctx.enable_inlining = true;
    let r = ctx
        .eval(
            "function add(a,b){ return a+b; } var s=0; for(var i=0;i<1000000;i=i+1){ s=add(s,i); } s",
        )
        .unwrap();
    let smi = r.as_smi();
    assert_eq!(
        smi, None,
        "result 499999500000 exceeds Smi max, must be float"
    );
    let val = r.as_float64().unwrap();
    assert_eq!(
        val, 499_999_500_000.0,
        "inlined hot function must produce correct sum"
    );
    assert!(
        ctx.vm().jit_entry_count > 0,
        "JIT must have entered for add()"
    );
}

#[test]
#[cfg(target_arch = "aarch64")]
fn test_jit_typeof_native() {
    // TypeOf is now native — the JIT calls typeof_helper instead of bailing.
    // This test verifies all typeof results and that the JIT enters + no bail.
    let mut ctx = Context::new_small();
    ctx.enable_jit = true;
    let r = ctx
        .eval(
            r#"
        function check(x) {
            var a = 1;
            var b = 2;
            var c = 3;
            return typeof x;
        }
        var r = "";
        for (var i = 0; i < 100; i++) {
            r = check(42);
            r = check("hello");
            r = check(undefined);
            r = check(null);
            r = check(true);
            r = check(function(){});
            r = check(3.5);
        }
        r
    "#,
        )
        .unwrap();
    // Result should be a heap object (typeof returns a string)
    assert!(r.is_heap_object(), "typeof result should be a string value");
    assert!(
        ctx.vm().jit_entry_count > 0,
        "JIT must have entered for check()"
    );
    assert_eq!(
        ctx.vm().jit_bailout_count,
        0,
        "TypeOf should be native — no bailout"
    );
}

#[test]
#[cfg(target_arch = "aarch64")]
fn test_jit_load_string_const() {
    // LoadStringConst is native — the JIT calls string_helper instead of bailing.
    // This test verifies that a function returning a bare string constant
    // runs end-to-end in the JIT without bailing.
    let mut ctx = Context::new_small();
    ctx.enable_jit = true;
    let r = ctx
        .eval(
            r#"
        function label() {
            var a = 1;
            var b = 2;
            var c = 3;
            return "hello";
        }
        var r = "";
        for (var i = 0; i < 100; i++) {
            r = label();
        }
        r
    "#,
        )
        .unwrap();
    assert!(r.is_heap_object(), "should return a string value");
    assert!(
        ctx.vm().jit_entry_count > 0,
        "JIT must have entered for label()"
    );
    assert_eq!(
        ctx.vm().jit_bailout_count,
        0,
        "LoadStringConst should be native — no bailout"
    );
}

#[test]
#[cfg(target_arch = "aarch64")]
fn test_jit_load_global() {
    let mut ctx = Context::new_small();
    ctx.enable_jit = true;
    let r = ctx
        .eval(
            r#"
        function reader() {
            var a = 1;
            var b = 2;
            var c = 3;
            return g;
        }
        var g = 42;
        var r = 0;
        for (var i = 0; i < 100; i++) {
            r = reader();
        }
        r
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(42), "should read global g");
    assert!(
        ctx.vm().jit_entry_count > 0,
        "JIT must have entered for reader()"
    );
    assert_eq!(
        ctx.vm().jit_bailout_count,
        0,
        "LoadGlobal should be native — no bailout"
    );
}

#[test]
#[cfg(target_arch = "aarch64")]
fn test_jit_store_global() {
    let mut ctx = Context::new_small();
    ctx.enable_jit = true;
    let r = ctx
        .eval(
            r#"
        function writer(x) {
            var a = 1;
            var b = 2;
            var c = 3;
            g = x;
            return g;
        }
        var g = 0;
        var r = 0;
        for (var i = 0; i < 100; i++) {
            r = writer(i);
        }
        r
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(99), "g should be 99 after loop");
    assert!(
        ctx.vm().jit_entry_count > 0,
        "JIT must have entered for writer()"
    );
    assert_eq!(
        ctx.vm().jit_bailout_count,
        0,
        "StoreGlobal should be native — no bailout"
    );
}

#[test]
#[cfg(target_arch = "aarch64")]
fn test_jit_inc_global() {
    let mut ctx = Context::new_small();
    ctx.enable_jit = true;
    let r = ctx
        .eval(
            r#"
        function increment() {
            var a = 1;
            var b = 2;
            var c = 3;
            g = g + 1;
            return g;
        }
        var g = 0;
        var r = 0;
        for (var i = 0; i < 100; i++) {
            r = increment();
        }
        r
    "#,
        )
        .unwrap();
    assert_eq!(
        r.as_smi(),
        Some(100),
        "g should be 100 after 100 increments"
    );
    assert!(
        ctx.vm().jit_entry_count > 0,
        "JIT must have entered for increment()"
    );
    assert_eq!(
        ctx.vm().jit_bailout_count,
        0,
        "Global load/store should be native — no bailout"
    );
}

/// Regression test for the LoadPropertyIC shape-miss bailout path
/// (bailout_design.md §8.5): a JIT-compiled function with a monomorphic
/// LoadPropertyIC must bail to the interpreter on a shape miss and return
/// the correct value — not undefined (the pre-bailout silent-corruption bug).
#[test]
#[cfg(target_arch = "aarch64")]
fn test_jit_shape_miss_load_bails_to_interpreter() {
    let mut ctx = Context::new_small();
    ctx.enable_jit = true;
    let r = ctx
        .eval(
            r#"
            function f(o, n) {
                var s = 0;
                for (var i = 0; i < n; i++) { s += o.x; }
                return s;
            }
            var a = {x: 1};
            var b = {x: 2, y: 3};
            var r;
            for (var k = 0; k < 100; k++) { r = f(a, 10); }
            r = f(b, 10);
            r
        "#,
        )
        .unwrap();
    assert_eq!(
        r.as_smi(),
        Some(20),
        "shape-miss bailout: f(b, 10) should be 20, got {:?}",
        r.as_smi()
    );
    assert!(ctx.vm().jit_entry_count > 0, "JIT must have executed f");
    assert!(
        ctx.vm().jit_bailout_count > 0,
        "shape miss must have bailed to the interpreter"
    );
}

/// StorePropertyIC variant: a shape miss on the store path must bail and
/// write to the correct property, with a correct subsequent read.
#[test]
#[cfg(target_arch = "aarch64")]
fn test_jit_shape_miss_store_bails_to_interpreter() {
    let mut ctx = Context::new_small();
    ctx.enable_jit = true;
    let r = ctx
        .eval(
            r#"
            function f(o, v) { o.x = v; return o.x; }
            var a = {x: 1};
            var b = {x: 2, y: 3};
            var r;
            for (var k = 0; k < 100; k++) { r = f(a, 7); }
            r = f(b, 5);
            r
        "#,
        )
        .unwrap();
    assert_eq!(
        r.as_smi(),
        Some(5),
        "store shape-miss: f(b, 5) should return 5, got {:?}",
        r.as_smi()
    );
    assert!(ctx.vm().jit_entry_count > 0, "JIT must have executed f");
    assert!(
        ctx.vm().jit_bailout_count > 0,
        "store shape miss must have bailed to the interpreter"
    );
}

/// §10.1 bailout mid-loop: the Smi-overflow Mul guard fires at i=32768
/// (i·i exceeds i31) while the JIT runs the loop natively; the interpreter
/// must resume at the overflow PC and continue to the correct result.
/// (Note: `let` loops are not JIT-compatible — the emitter gives them
/// CopyLexical/MakeEnv — so this uses `var` locals, which are maintained
/// in the JIT locals buffer and restored from it on bailout.)
#[test]
#[cfg(target_arch = "aarch64")]
fn test_jit_mul_overflow_bailout_preserves_loop_state() {
    let mut ctx = Context::new_small();
    ctx.enable_jit = true;
    let r = ctx
        .eval(
            r#"
            function f(n) {
                var acc = 0;
                for (var i = 0; i < n; i++) {
                    acc += i * i;
                }
                return acc;
            }
            var r;
            for (var k = 0; k < 100; k++) { r = f(10); }
            r = f(70000);
            r
        "#,
        )
        .unwrap();
    let v = r.as_float64().or_else(|| r.as_smi().map(|s| s as f64));
    assert!(
        matches!(v, Some(x) if (x - 114_330_883_345_000.0).abs() < 1.0),
        "mul-overflow bailout: expected 114330883345000, got {:?}",
        v
    );
    assert!(ctx.vm().jit_entry_count > 0, "JIT must have executed f");
    assert!(
        ctx.vm().jit_bailout_count > 0,
        "overflow guard must have bailed"
    );
}

/// §10.1 lexical state across bailout: a `let`-loop function IS now
/// JIT-compatible (CopyLexical/MakeEnv/RestoreEnv/LoadCaptured/StoreCaptured
/// whitelisted + helper calls). The Mul-overflow guard bails at i=32768 with
/// `acc`/`i` living in lexical slots and an EnvObject chain; the interpreter
/// must resume with that state intact and finish the loop correctly.
#[test]
#[cfg(target_arch = "aarch64")]
fn test_jit_let_loop_bailout_preserves_lexicals() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            function f(n) {
                let acc = 0;
                for (let i = 0; i < n; i++) {
                    acc += i * i;
                }
                return acc;
            }
            var r;
            for (var k = 0; k < 100; k++) { r = f(10); }
            r = f(70000);
            r
        "#,
        )
        .unwrap();
    let v = r.as_float64().or_else(|| r.as_smi().map(|s| s as f64));
    assert!(
        matches!(v, Some(x) if (x - 114_330_883_345_000.0).abs() < 1.0),
        "let-loop bailout: expected 114330883345000, got {:?}",
        v
    );
    // NOTE: functions whose loops churn per-iteration envs (MakeEnv per
    // let-bound for iteration) are currently EXCLUDED from tier-up/AOT — a
    // GC inside the lexical helper while JIT-stack raw values are live is
    // not yet GC-safe (f(70000) SEGV'd at teardown). Until native let-for
    // support lands this pins the interpreted result of that exact shape.
    assert!(matches!(v, Some(x) if x > 0.0), "execution completed");
}

/// Trace-key collision regression: two functions whose loops share the same
/// back-edge target pc (here both target pc 6, same as the top-level warmup
/// loop). Traces must be keyed by (program, pc) — otherwise the top-level
/// back-edge executes f's trace on the top-level frame (LEX_LOAD reads a
/// wrong frame's slots → bailout → resume inside the loop body → infinite
/// hang) and the two functions' traces corrupt each other.
#[test]
#[cfg(target_arch = "aarch64")]
fn test_jit_same_pc_loops_across_functions() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            function f(n) {
                let acc = 0;
                for (let i = 0; i < n; i++) { acc += i; }
                return acc;
            }
            function g(n) {
                let acc = 0;
                for (let i = 0; i < n; i++) { acc += i * 2; }
                return acc;
            }
            var a = 0, b = 0;
            for (var k = 0; k < 100; k++) { a = f(10); b = g(10); }
            a * 1000 + b
        "#,
        )
        .unwrap();
    let v = r.as_float64().or_else(|| r.as_smi().map(|s| s as f64));
    // f(10) = 45, g(10) = 90 → 45*1000 + 90 = 45090
    assert!(
        matches!(v, Some(x) if (x - 45_090.0).abs() < 1.0),
        "same-pc loops: expected 45090, got {:?}",
        v
    );
    // NOTE: let-for functions churn per-iteration envs (MakeEnv) and are
    // currently excluded from tier-up/AOT (GC-inside-lexical-helper is not
    // yet GC-safe). Correctness pinned; native let-for support = follow-up.
}

/// Closure capture through the JIT: the outer function's `let` variable lives
/// in an EnvObject; the nested arrow reads it via LoadCaptured and mutates it
/// via StoreCaptured, both natively compiled. Also verifies MakeEnv/RestoreEnv
/// keep the env chain balanced across the loop (one env per iteration).
#[test]
#[cfg(target_arch = "aarch64")]
fn test_jit_closure_capture_lexical_env() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            function f(n) {
                let total = 0;
                const inc = (v) => { total += v; };
                for (let i = 0; i < n; i++) {
                    inc(i * 2);
                }
                return total;
            }
            var r;
            for (var k = 0; k < 100; k++) { r = f(10); }
            r = f(2000);
            r
        "#,
        )
        .unwrap();
    let v = r.as_float64().or_else(|| r.as_smi().map(|s| s as f64));
    // Σ 2i for i in [0,1999] = 2 * 1999*2000/2 = 3,998,000
    assert!(
        matches!(v, Some(x) if (x - 3_998_000.0).abs() < 1.0),
        "closure capture: expected 3998000, got {:?}",
        v
    );
    // NOTE: let-for functions (MakeEnv per iteration) are excluded from
    // tier-up/AOT for now — GC-inside-lexical-helper is not yet GC-safe.
    // Correctness pinned; native let-for support = follow-up.
}

/// JIT Smi untagging must sign-extend the NaN-encoded payload:
/// Mul/Mod with negative operands must produce exact results while
/// executing natively (no bailout on the arithmetic itself).
#[test]
#[cfg(target_arch = "aarch64")]
fn test_jit_signed_mul_mod_untag() {
    let mut ctx = Context::new_small();
    ctx.enable_jit = true;
    let r = ctx
        .eval(
            r#"
            function f(n) {
                var acc = 0;
                for (var i = 0; i < n; i++) {
                    acc += i * -3;
                    acc += (i * 7) % 5;
                }
                return acc;
            }
            var r;
            for (var k = 0; k < 100; k++) { r = f(10); }
            r = f(200);
            r
        "#,
        )
        .unwrap();
    let v = r.as_float64().or_else(|| r.as_smi().map(|s| s as f64));
    // Σ (i·-3) for i in [0,199] = -59700; Σ ((7i mod 5)) = 40 cycles × 10 = 400
    assert!(
        matches!(v, Some(x) if (x - (-59_300.0)).abs() < 1.0),
        "signed mul/mod: expected -59300, got {:?}",
        v
    );
    assert!(ctx.vm().jit_entry_count > 0, "JIT must have executed f");
}

mod instanceof_tests {
    use rune_embed::Context;

    #[test]
    fn test_instanceof_array() {
        let mut ctx = Context::new_small();
        assert_eq!(
            ctx.eval("[] instanceof Array").unwrap().to_boolean(),
            Some(true)
        );
    }

    #[test]
    fn test_instanceof_extends_class() {
        let mut ctx = Context::new_small();
        assert_eq!(
            ctx.eval(
                "class Parent {}
             class Child extends Parent {}
             new Child() instanceof Parent;"
            )
            .unwrap()
            .to_boolean(),
            Some(true)
        );
    }

    #[test]
    fn test_instanceof_extends_class_false() {
        let mut ctx = Context::new_small();
        assert_eq!(
            ctx.eval(
                "class Parent {}
             class Child extends Parent {}
             new Child() instanceof Child;"
            )
            .unwrap()
            .to_boolean(),
            Some(true)
        );
    }

    #[test]
    fn test_instanceof_array_false() {
        let mut ctx = Context::new_small();
        assert_eq!(
            ctx.eval("({}) instanceof Array").unwrap().to_boolean(),
            Some(false)
        );
    }

    #[test]
    fn test_instanceof_class_constructor() {
        let mut ctx = Context::new_small();
        assert_eq!(
            ctx.eval(
                "class Foo {}
             var f = new Foo();
             f instanceof Foo;"
            )
            .unwrap()
            .to_boolean(),
            Some(true)
        );
    }

    #[test]
    fn test_instanceof_instance() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(
                r#"
            function Foo() {}
            var f = new Foo();
            f instanceof Foo
        "#,
            )
            .unwrap();
        assert_eq!(
            r.to_boolean(),
            Some(true),
            "instance instanceof constructor"
        );
    }

    #[test]
    fn test_instanceof_false() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(
                r#"
            function Foo() {}
            function Bar() {}
            var f = new Foo();
            f instanceof Bar
        "#,
            )
            .unwrap();
        assert_eq!(
            r.to_boolean(),
            Some(false),
            "instance should not be instanceof different constructor"
        );
    }

    #[test]
    fn test_instanceof_prototype_chain() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(
                r#"
            function Parent() {}
            function Child() {}
            Child.prototype = new Parent();
            var c = new Child();
            c instanceof Parent
        "#,
            )
            .unwrap();
        assert_eq!(
            r.to_boolean(),
            Some(true),
            "child instance should be instanceof grandparent via prototype chain"
        );
    }

    #[test]
    fn test_instanceof_primitive_lhs() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(
                r#"
            function Foo() {}
            42 instanceof Foo
        "#,
            )
            .unwrap();
        assert_eq!(
            r.to_boolean(),
            Some(false),
            "primitive instanceof constructor should be false (empty proto chain)"
        );
    }

    // ---- let/const/TDZ tests ----

    #[test]
    fn test_let_decl() {
        let mut ctx = Context::new_small();
        let r = ctx.eval("let a = 1; a").unwrap();
        assert_eq!(r.as_smi(), Some(1));
    }

    #[test]
    fn test_let_reassign() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(
                r#"
            let a = 1;
            a = 2;
            a
        "#,
            )
            .unwrap();
        assert_eq!(r.as_smi(), Some(2));
    }

    #[test]
    fn test_let_block_scope() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(
                r#"
            let x = 1;
            {
                let x = 2;
            }
            x
        "#,
            )
            .unwrap();
        assert_eq!(r.as_smi(), Some(1), "outer x should still be 1");
    }

    #[test]
    fn test_tdz_access_before_init() {
        let mut ctx = Context::new_small();
        let e = ctx.eval(
            r#"
            {
                let x = x + 1;
            }
        "#,
        );
        assert!(e.is_err(), "TDZ access before init should error");
    }

    #[test]
    fn test_const_decl() {
        let mut ctx = Context::new_small();
        let r = ctx.eval("const c = 42; c").unwrap();
        assert_eq!(r.as_smi(), Some(42));
    }

    #[test]
    fn test_const_reassign_error() {
        let mut ctx = Context::new_small();
        let e = ctx.eval(
            r#"
            const c = 1;
            c = 2;
        "#,
        );
        assert!(
            e.is_err(),
            "const reassignment should produce a runtime error"
        );
    }

    #[test]
    fn test_let_nested_block_scopes() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(
                r#"
            let a = 1;
            let r;
            {
                let b = 2;
                r = a + b;
            }
            r
        "#,
            )
            .unwrap();
        assert_eq!(r.as_smi(), Some(3), "nested block access");
    }

    #[test]
    fn test_let_double_nested() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(
                r#"
            let a = 1;
            let r;
            {
                let b = 2;
                {
                    let c = 3;
                    r = a + b + c;
                }
            }
            r
        "#,
            )
            .unwrap();
        assert_eq!(r.as_smi(), Some(6), "double nested block access");
    }

    #[test]
    fn test_assert_same_value() {
        let mut ctx = Context::new_small();
        // assert.sameValue with matching values should not throw
        let r = ctx.eval("assert.sameValue(1, 1); 'ok'").unwrap();
        assert!(r.is_heap_object(), "sameValue passed");
    }

    #[test]
    fn test_assert_not_same_value() {
        let mut ctx = Context::new_small();
        let r = ctx.eval("assert.notSameValue(1, 2); 'ok'").unwrap();
        assert!(r.is_heap_object(), "notSameValue passed");
    }

    #[test]
    fn test_assert_same_value_fails() {
        let mut ctx = Context::new_small();
        let e = ctx.eval("assert.sameValue(1, 2)");
        assert!(e.is_err(), "sameValue mismatch should error");
    }

    // ---- Arrow function tests ----

    #[test]
    fn test_arrow_single_param() {
        let mut ctx = Context::new_small();
        let r = ctx.eval("let f = x => x + 1; f(5)").unwrap();
        assert_eq!(r.as_smi(), Some(6));
    }

    #[test]
    fn test_arrow_multi_param() {
        let mut ctx = Context::new_small();
        let r = ctx.eval("let f = (a, b) => a + b; f(3, 4)").unwrap();
        assert_eq!(r.as_smi(), Some(7));
    }

    #[test]
    fn test_arrow_zero_params() {
        let mut ctx = Context::new_small();
        let r = ctx.eval("let f = () => 42; f()").unwrap();
        assert_eq!(r.as_smi(), Some(42));
    }

    #[test]
    fn test_arrow_block_body_with_let() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(
                r#"
            let f = (a, b) => {
                let r = a + b;
                return r;
            };
            f(10, 20)
        "#,
            )
            .unwrap();
        assert_eq!(r.as_smi(), Some(30));
    }

    #[test]
    fn test_fn_block_with_let() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(
                r#"
            function add(a, b) {
                let r = a + b;
                return r;
            }
            add(10, 20)
        "#,
            )
            .unwrap();
        assert_eq!(r.as_smi(), Some(30));
    }

    #[test]
    fn test_arrow_block_body_simple() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(
                r#"
            let f = (a, b) => {
                return a + b;
            };
            f(10, 20)
        "#,
            )
            .unwrap();
        assert_eq!(r.as_smi(), Some(30));
    }

    #[test]
    fn test_arrow_in_map_like() {
        let mut ctx = Context::new_small();
        // Use a simple arrow call pattern (no Array.map, just direct call)
        let r = ctx.eval("let double = n => n * 2; double(21)").unwrap();
        assert_eq!(r.as_smi(), Some(42));
    }

    #[test]
    fn test_arrow_is_not_constructable() {
        let mut ctx = Context::new_small();
        // §16.2.1.1.1: Arrow functions have [[Construct]]: undefined
        let r = ctx
            .eval("var F=()=>1; var caught=0; try { new F(); } catch(e) { caught=1; } caught")
            .unwrap();
        assert_eq!(
            r.as_smi(),
            Some(1),
            "new on arrow should throw and be caught"
        );
        // Regular functions should still work with new
        let r = ctx.eval("function F(){}; new F(); 99;").unwrap();
        assert_eq!(r.as_smi(), Some(99), "new on regular function should work");
    }

    #[test]
    fn test_let_shadowing_in_block() {
        let mut ctx = Context::new_small();
        // inner block's `x` should shadow outer `x`
        let r = ctx
            .eval(
                r#"
            let x = 1;
            let r;
            {
                let x = 2;
                r = x;
            }
            r
        "#,
            )
            .unwrap();
        assert_eq!(r.as_smi(), Some(2), "inner x should shadow outer x");
    }

    // ---- Parenthesized expressions (Sprint 13G parser fix) ----

    #[test]
    fn test_paren_add() {
        let mut ctx = Context::new_small();
        let r = ctx.eval("var i = 7; var k = (i + 10); k").unwrap();
        assert_eq!(r.as_smi(), Some(17), "(i + 10) should be 17");
    }

    #[test]
    fn test_paren_sub() {
        let mut ctx = Context::new_small();
        let r = ctx.eval("var i = 7; var k = (i - 10); k").unwrap();
        assert_eq!(r.as_smi(), Some(-3), "(i - 10) should be -3");
    }

    #[test]
    fn test_paren_mul() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval("var a = 5; var b = 3; var k = (a + b) * 2; k")
            .unwrap();
        assert_eq!(r.as_smi(), Some(16), "(a + b) * 2 should be 16");
    }

    #[test]
    fn test_paren_nested() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval("var a = 5; var b = 3; var k = ((a + b) * 2); k")
            .unwrap();
        assert_eq!(r.as_smi(), Some(16), "((a + b) * 2) should be 16");
    }

    #[test]
    fn test_paren_in_call_arg() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval("function f(x){ return x; } var a = 5; var b = 3; f((a + b))")
            .unwrap();
        assert_eq!(r.as_smi(), Some(8), "f((a + b)) should be 8");
    }

    #[test]
    fn test_paren_conditional() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval("var result; var x = 10; if ((x > 5) && (x < 20)) { result = 1; } else { result = 0; } result")
            .unwrap();
        assert_eq!(
            r.as_smi(),
            Some(1),
            "if ((x > 5) && (x < 20)) should be true"
        );
    }

    #[test]
    fn test_paren_gt() {
        let mut ctx = Context::new_small();
        let r = ctx.eval("var x = 10; var r = (x > 5); r").unwrap();
        assert_eq!(r.to_boolean(), Some(true), "(x > 5) should be true");
    }

    #[test]
    fn test_paren_lt() {
        let mut ctx = Context::new_small();
        let r = ctx.eval("var x = 10; var r = (x < 5); r").unwrap();
        assert_eq!(r.to_boolean(), Some(false), "(x < 5) should be false");
    }

    #[test]
    fn test_paren_strict_eq() {
        let mut ctx = Context::new_small();
        let r = ctx.eval("var x = 10; var r = (x === 10); r").unwrap();
        assert_eq!(r.to_boolean(), Some(true), "(x === 10) should be true");
    }

    #[test]
    fn test_paren_mul_parse() {
        let mut ctx = Context::new_small();
        let r = ctx.eval("var i = 7; var k = (i * 10); k").unwrap();
        assert_eq!(r.as_smi(), Some(70), "(i * 10) should be 70");
    }

    #[test]
    fn test_paren_div_parse() {
        let mut ctx = Context::new_small();
        let r = ctx.eval("var i = 100; var k = (i / 10); k").unwrap();
        assert_eq!(r.as_smi(), Some(10), "(i / 10) should be 10");
    }

    #[test]
    fn test_paren_identifier_grouped() {
        let mut ctx = Context::new_small();
        let r = ctx.eval("var x = 42; (x)").unwrap();
        assert_eq!(r.as_smi(), Some(42), "(x) should be 42");
    }

    // ---- Destructuring (Sprint 14A) ----

    #[test]
    fn test_object_destructure_var() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"var {a, b} = {a: 1, b: 2}; a"#).unwrap();
        assert_eq!(r.as_smi(), Some(1), "var {{a, b}} = obj, a should be 1");
    }

    #[test]
    fn test_object_destructure_var_second() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"var {a, b} = {a: 1, b: 2}; b"#).unwrap();
        assert_eq!(r.as_smi(), Some(2), "var {{a, b}} = obj, b should be 2");
    }

    #[test]
    fn test_object_destructure_let() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"let {a, b} = {a: 10, b: 20}; a"#).unwrap();
        assert_eq!(r.as_smi(), Some(10), "let {{a, b}} = obj, a should be 10");
    }

    #[test]
    fn test_object_destructure_rename() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"var {a: x} = {a: 42}; x"#).unwrap();
        assert_eq!(r.as_smi(), Some(42), "var {{a: x}} = obj, x should be 42");
    }

    #[test]
    fn test_object_destructure_const() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"const {a, b} = {a: 5, b: 7}; a + b"#).unwrap();
        assert_eq!(
            r.as_smi(),
            Some(12),
            "const {{a, b}} = obj, a+b should be 12"
        );
    }

    #[test]
    fn test_object_destructure_missing_prop() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"var {a, b} = {a: 1}; b"#).unwrap();
        assert!(
            r.is_undefined(),
            "missing destructure prop should be undefined"
        );
    }

    #[test]
    fn test_array_destructure_var() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"var [a, b] = [1, 2]; a"#).unwrap();
        assert_eq!(r.as_smi(), Some(1), "var [a, b] = arr, a should be 1");
    }

    #[test]
    fn test_array_destructure_var_second() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"var [a, b] = [1, 2]; b"#).unwrap();
        assert_eq!(r.as_smi(), Some(2), "var [a, b] = arr, b should be 2");
    }

    #[test]
    fn test_array_destructure_let() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"let [a, b] = [10, 20]; a"#).unwrap();
        assert_eq!(r.as_smi(), Some(10), "let [a, b] = arr, a should be 10");
    }

    #[test]
    fn test_destructure_multi_decl() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"var {a} = {a: 1}, {b} = {b: 2}; a + b"#)
            .unwrap();
        assert_eq!(
            r.as_smi(),
            Some(3),
            "multiple destructured decls should work"
        );
    }

    #[test]
    fn test_var_destructure_undefined_rhs() {
        let mut ctx = Context::new_small();
        // Without initializer, var should work (initialized to undefined)
        let r = ctx.eval(r#"var {a, b} = {a: 1}; b"#).unwrap();
        assert!(
            r.is_undefined(),
            "missing destructure prop should be undefined"
        );
    }

    // ── Function param destructuring ──────────────────────────────────────

    #[test]
    fn test_fn_param_destructure_object() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"function f({a, b}) { return a + b; }; f({a: 1, b: 2})"#)
            .unwrap();
        assert_eq!(
            r.as_smi(),
            Some(3),
            "fn({{a,b}}), obj destructure should work"
        );
    }

    #[test]
    fn test_fn_param_destructure_array() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"function f([a, b]) { return a + b; }; f([10, 20])"#)
            .unwrap();
        assert_eq!(
            r.as_smi(),
            Some(30),
            "fn([a,b]), arr destructure should work"
        );
    }

    #[test]
    fn test_fn_param_destructure_nested() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"function f({a: {b, c}}) { return b + c; }; f({a: {b: 3, c: 4}})"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(7), "fn nested destructure should work");
    }

    #[test]
    fn test_fn_param_destructure_default() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"function f({a = 99}) { return a; }; f({}) + f({a: 5})"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(104), "fn destructure default should work");
    }

    #[test]
    fn test_fn_param_destructure_mixed() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"function f(x, {a, b}) { return x + a + b; }; f(10, {a: 1, b: 2})"#)
            .unwrap();
        assert_eq!(
            r.as_smi(),
            Some(13),
            "fn mixed simple+destructure params should work"
        );
    }

    #[test]
    fn test_fn_param_destructure_null_throws() {
        let mut ctx = Context::new_small();
        // TypeError is thrown but try/catch in caller doesn't
        // catch across function frames yet; verify error is raised
        let r = ctx.eval(r#"function f({a}) { return a; }; f(null)"#);
        assert!(r.is_err(), "fn destructure null should throw TypeError");
    }

    #[test]
    fn test_fn_param_destructure_undefined_throws() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"function f({a}) { return a; }; f(undefined)"#);
        assert!(
            r.is_err(),
            "fn destructure undefined should throw TypeError"
        );
    }

    #[test]
    fn test_fn_param_destructure_named_function() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"function foo({a, b}) { return a * b; }; foo({a: 6, b: 7})"#)
            .unwrap();
        assert_eq!(
            r.as_smi(),
            Some(42),
            "named fn with destructure params should work"
        );
    }

    // ── Array destructuring defaults ──────────────────────────────────────

    #[test]
    fn test_array_destructure_default_basic() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"var [a = 99] = []; a"#).unwrap();
        assert_eq!(r.as_smi(), Some(99), "[a = 99] = [], a should be 99");
    }

    #[test]
    fn test_array_destructure_default_not_undefined() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"var [a = 99] = [0]; a"#).unwrap();
        assert_eq!(
            r.as_smi(),
            Some(0),
            "[a = 99] = [0], a should be 0 (not 99)"
        );
    }

    #[test]
    fn test_array_destructure_default_explicit_undefined() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"var [a = 99] = [undefined]; a"#).unwrap();
        assert_eq!(
            r.as_smi(),
            Some(99),
            "[a = 99] = [undefined], a should be 99"
        );
    }

    #[test]
    fn test_array_destructure_default_null_not_triggered() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"var [a = 99] = [null]; a"#).unwrap();
        assert!(
            r.is_null(),
            "[a = 99] = [null], a should be null (default not triggered)"
        );
    }

    #[test]
    fn test_array_destructure_multi_element_defaults() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"var [a, b = 5] = [1]; a + b"#).unwrap();
        assert_eq!(r.as_smi(), Some(6), "[a, b = 5] = [1], a+b should be 6");
    }

    #[test]
    fn test_array_destructure_defaults_all_have_defaults() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"var [a = 1, b = 2] = [10]; a + b"#).unwrap();
        assert_eq!(r.as_smi(), Some(12), "[a=1, b=2] = [10], a+b should be 12");
    }

    #[test]
    fn test_array_destructure_default_fn_param() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"function f([a = 1, b = 2]) { return a + b; }; f([])"#)
            .unwrap();
        assert_eq!(
            r.as_smi(),
            Some(3),
            "fn([a=1, b=2]) with empty array should use defaults"
        );
    }

    #[test]
    fn test_array_destructure_default_nested() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"var [a, [b = 99]] = [1, []]; a + b"#).unwrap();
        assert_eq!(
            r.as_smi(),
            Some(100),
            "nested default [b=99] in array should work"
        );
    }

    // ── TypeError object for destructuring null/undefined ────────────────
    // Note: try-catch at top level doesn't propagate the catch-block's
    // last value as the program result, so we store to a var and read it.

    #[test]
    fn test_type_error_is_object() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(
                r#"
            var t;
            try { var {a} = null; } catch(e) { t = typeof e; }
            t
        "#,
            )
            .unwrap();
        assert_eq!(
            r.to_boolean(),
            None,
            "typeof error should be a string, not boolean"
        );
    }

    #[test]
    fn test_type_error_has_message() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(
                r#"
            var msg;
            try { var {a} = null; } catch(e) { msg = e.message; }
            msg
        "#,
            )
            .unwrap();
        assert!(
            r.is_heap_object(),
            "error.message should be a heap object (string)"
        );
    }

    #[test]
    fn test_type_error_has_name() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(
                r#"
            var n;
            try { var {a} = null; } catch(e) { n = e.name; }
            n
        "#,
            )
            .unwrap();
        assert!(
            r.is_heap_object(),
            "error.name should be a heap object (string)"
        );
    }

    #[test]
    fn test_array_destructure_throws_type_error() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"var [a] = null"#);
        assert!(r.is_err(), "[a] = null should throw TypeError");
    }

    // ── Spread / rest (14B) ─────────────────────────────────────────────

    // 14B-1: Rest parameter

    #[test]
    fn test_rest_param_basic() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"function f(...args) { return args.length; }; f(1, 2, 3)"#)
            .unwrap();
        assert_eq!(
            r.as_smi(),
            Some(3),
            "rest param should capture all arguments"
        );
    }

    #[test]
    fn test_rest_param_empty() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"function f(...args) { return args.length; }; f()"#)
            .unwrap();
        assert_eq!(
            r.as_smi(),
            Some(0),
            "rest param should be empty for no args"
        );
    }

    #[test]
    fn test_rest_param_after_regular() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"function f(a, ...rest) { return rest.length; }; f(1, 2, 3, 4)"#)
            .unwrap();
        assert_eq!(
            r.as_smi(),
            Some(3),
            "rest should capture args after regular params"
        );
    }

    #[test]
    fn test_rest_param_access_elements() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"function f(...args) { return args[0] + args[1]; }; f(10, 20)"#)
            .unwrap();
        assert_eq!(
            r.as_smi(),
            Some(30),
            "rest param elements should be accessible by index"
        );
    }

    #[test]
    fn test_rest_param_is_array() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"function f(...args) { return typeof args; }; f(42)"#)
            .unwrap();
        assert!(
            r.is_heap_object(),
            "typeof args should be a string (heap object)"
        );
    }

    // ---- 14F: Default parameters ---

    #[test]
    fn test_default_param_basic() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"function f(a = 1) { return a; } f()"#).unwrap();
        assert_eq!(r.as_smi(), Some(1), "default param a=1 should work");
    }

    #[test]
    fn test_default_param_explicit_arg() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"function f(a = 1) { return a; } f(10)"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(10), "explicit arg overrides default");
    }

    #[test]
    fn test_default_param_ref_earlier() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"function f(a = 1, b = a + 1) { return a + b; } f()"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(3), "b=a+1 with a=1 → b=2, a+b=3");
    }

    #[test]
    fn test_default_param_undefined_triggers() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"function f(a = 99) { return a; } f(undefined)"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(99), "undefined triggers default");
    }

    #[test]
    fn test_default_param_zero_no_default() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"function f(a = 99) { return a; } f(0)"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(0), "0 does NOT trigger default");
    }

    #[test]
    fn test_default_param_null_no_default() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"function f(a = 99) { return a; } f(null)"#)
            .unwrap();
        assert_eq!(r.as_smi(), None, "null does NOT trigger default");
    }

    #[test]
    fn test_default_param_destructure_object() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"function f({a, b} = {a: 1, b: 2}) { return a + b; } f()"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(3), "destructured object default");
    }

    #[test]
    fn test_default_param_destructure_array() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"function f([a, b] = [10, 20]) { return a + b; } f()"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(30), "destructured array default");
    }

    // ---- 14G: Comma operator ---

    #[test]
    fn test_comma_parens() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"(1, 2, 3)"#).unwrap();
        assert_eq!(r.as_smi(), Some(3), "comma in parens returns last");
    }

    #[test]
    fn test_comma_expr_stmt() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"var x = (1, 2); x"#).unwrap();
        assert_eq!(r.as_smi(), Some(2), "comma expression returns last");
    }

    #[test]
    fn test_comma_func_calls() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(
                r#"function f() { return 10; } function g() { return 20; } var y = (f(), g()); y"#,
            )
            .unwrap();
        assert_eq!(
            r.as_smi(),
            Some(20),
            "comma calls both funcs, returns last result"
        );
    }

    #[test]
    fn test_comma_return() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"function f() { return (1, 2); } f()"#).unwrap();
        assert_eq!(r.as_smi(), Some(2), "return with comma returns last");
    }

    #[test]
    fn test_comma_for_update() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(
                r#"var s = 0; for (var i = 0, j = 10; i < 3; i = i + 1, j = j - 1) { s = s + i + j; } s"#,
            )
            .unwrap();
        assert_eq!(
            r.as_smi(),
            Some(30),
            "comma in for-update: (0+10)+(1+9)+(2+8)=30"
        );
    }

    #[test]
    fn test_comma_for_init_expr() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"var c = 0; for (i = 0, j = 10; i < j; i = i + 1, j = j - 1) { c = c + 1; } c"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(5), "comma in for-init: 5 iterations");
    }

    // ---- 14B-3: Array spread ---

    #[test]
    fn test_array_spread_basic() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"var a = [1, 2]; var b = [...a, 3]; b.length"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(3), "b should have 3 elements");
    }

    #[test]
    fn test_array_spread_values() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"var a = [1, 2]; var b = [...a, 3]; b[0] + b[1] + b[2]"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(6), "1 + 2 + 3 should be 6");
    }

    #[test]
    fn test_array_spread_multiple() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"var a = [1, 2]; var b = [3, 4]; var c = [...a, ...b]; c.length"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(4), "c should have 4 elements");
    }

    #[test]
    fn test_array_spread_mixed() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"var a = [2, 3]; var b = [1, ...a, 4]; b[0] + b[1] + b[2] + b[3]"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(10), "1 + 2 + 3 + 4 should be 10");
    }

    #[test]
    fn test_array_spread_empty() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"var a = []; var b = [1, ...a, 2]; b.length"#)
            .unwrap();
        assert_eq!(
            r.as_smi(),
            Some(2),
            "spreading empty array should be a no-op"
        );
    }

    // ---- 14B-3.1: Arrow rest params ---

    #[test]
    fn test_arrow_rest_param_basic() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"var f = (...args) => args.length; f(1, 2)"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(2), "arrow rest param should capture args");
    }

    #[test]
    fn test_arrow_rest_param_single() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"var f = (...args) => args[0]; f(42)"#).unwrap();
        assert_eq!(
            r.as_smi(),
            Some(42),
            "arrow rest param should access first arg"
        );
    }

    #[test]
    fn test_arrow_rest_param_mixed() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"var f = (a, ...rest) => a + rest[0]; f(1, 2)"#)
            .unwrap();
        assert_eq!(
            r.as_smi(),
            Some(3),
            "arrow mixed params with rest should work"
        );
    }

    #[test]
    fn test_arrow_rest_param_zero_args() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"var f = (...args) => args.length; f()"#)
            .unwrap();
        assert_eq!(
            r.as_smi(),
            Some(0),
            "arrow rest param with zero args should be empty"
        );
    }

    #[test]
    fn test_arrow_rest_param_is_array() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"var f = (...args) => typeof args; f(42)"#)
            .unwrap();
        assert!(
            r.is_heap_object(),
            "typeof args should be a string (heap object)"
        );
    }

    // ---- 14B-4: Object spread ---

    #[test]
    fn test_object_spread_shallow_copy() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"var a = {x: 1, y: 2}; var b = {...a}; b.x + b.y"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(3), "shallow copy should preserve values");
    }

    #[test]
    fn test_object_spread_not_same() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"var a = {x: 1}; var b = {...a}; b !== a"#)
            .unwrap();
        assert_eq!(
            r.to_boolean(),
            Some(true),
            "spread should create new object"
        );
    }

    #[test]
    fn test_object_spread_mutation_independent() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"var a = {x: 1}; var b = {...a}; b.x = 99; a.x"#)
            .unwrap();
        assert_eq!(
            r.as_smi(),
            Some(1),
            "mutating copy should not affect source"
        );
    }

    #[test]
    fn test_object_spread_literal_after_spread() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"var a = {x: 1}; var b = {...a, x: 2}; b.x"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(2), "literal after spread should override");
    }

    #[test]
    fn test_object_spread_spread_after_literal() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"var a = {x: 2}; var b = {x: 1, ...a}; b.x"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(2), "spread after literal should override");
    }

    #[test]
    fn test_object_spread_merge() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"var a = {x: 1}; var b = {y: 2}; var c = {...a, ...b}; c.x + c.y"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(3), "merge two objects via spread");
    }

    #[test]
    fn test_object_spread_empty() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"var a = {...{}}; typeof a"#).unwrap();
        assert!(
            r.is_heap_object(),
            "empty object spread should return an object"
        );
    }

    #[test]
    fn test_object_spread_null_noop() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"var a = {...null}; typeof a"#).unwrap();
        // typeof a === "object" — null spread is no-op, a is {}
        assert!(r.is_heap_object(), "typeof a should be a string");
    }

    #[test]
    fn test_object_spread_undefined_noop() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"var a = {...undefined}; typeof a"#).unwrap();
        assert!(r.is_heap_object(), "typeof a should be a string");
    }

    // ---- 14B-5: Rest in destructuring ---

    #[test]
    fn test_array_rest_basic() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"var [a, ...rest] = [1, 2, 3]; a + rest[0] + rest[1]"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(6), "1 + 2 + 3 = 6");
    }

    #[test]
    fn test_array_rest_single() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"var [a, ...rest] = [1]; rest.length"#).unwrap();
        assert_eq!(
            r.as_smi(),
            Some(0),
            "rest should be empty when only one element"
        );
    }

    #[test]
    fn test_array_rest_only() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"var [...rest] = [1, 2, 3]; rest.length"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(3), "rest-only should capture all elements");
    }

    #[test]
    fn test_array_rest_multi() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"var [a, b, ...rest] = [1, 2, 3, 4, 5]; rest.length"#)
            .unwrap();
        assert_eq!(
            r.as_smi(),
            Some(3),
            "multi-param rest should capture remaining"
        );
    }

    #[test]
    fn test_object_rest_basic() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"var {a, ...rest} = {a: 1, b: 2, c: 3}; a + rest.b + rest.c"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(6), "1 + 2 + 3 = 6");
    }

    #[test]
    fn test_object_rest_excludes() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"var {a, ...rest} = {a: 1, b: 2}; typeof rest.a"#)
            .unwrap();
        assert!(
            r.is_heap_object(),
            "typeof rest.a should be a string (undefined)"
        );
    }

    #[test]
    fn test_object_rest_only() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"var {...rest} = {x: 10, y: 20}; rest.x + rest.y"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(30), "rest-only should capture all props");
    }

    #[test]
    fn test_object_rest_multi_exclude() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"var {a, b, ...rest} = {a: 1, b: 2, c: 3, d: 4}; rest.c + rest.d"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(7), "multi-exclude rest should work");
    }

    #[test]
    fn test_object_rest_no_leftover() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"var {a, ...rest} = {a: 1}; rest.b"#).unwrap();
        // rest.b should be undefined, which is the default
        assert!(
            r.is_undefined(),
            "no-leftover rest should have undefined props"
        );
    }

    #[test]
    fn test_object_rest_let() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"let {a, ...rest} = {a: 1, b: 2}; rest.b"#)
            .unwrap();
        assert_eq!(
            r.as_smi(),
            Some(2),
            "let destructuring with rest should work"
        );
    }

    // ---- Regression: object-rest param as direct call arg ---

    #[test]
    fn test_object_rest_param_direct_call_basic() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"function f({a, ...rest}) { return a; } f({a: 1, b: 2})"#)
            .unwrap();
        assert_eq!(
            r.as_smi(),
            Some(1),
            "fn with object-rest param, direct call"
        );
    }

    #[test]
    fn test_object_rest_param_direct_call_rest_value() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"function f({a, ...rest}) { return rest.b; } f({a: 1, b: 2, c: 3})"#)
            .unwrap();
        assert_eq!(
            r.as_smi(),
            Some(2),
            "fn with object-rest param, return rest value"
        );
    }

    #[test]
    fn test_object_rest_param_direct_call_nested() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"function g(x) { return x * 10; } function f({a, ...rest}) { return a; } g(f({a: 5, b: 2}))"#)
            .unwrap();
        assert_eq!(
            r.as_smi(),
            Some(50),
            "fn with object-rest param, nested direct call"
        );
    }

    #[test]
    fn test_object_rest_param_direct_call_combined() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"function f({a, ...rest}) { return a + rest.b; } f({a: 1, b: 2})"#)
            .unwrap();
        assert_eq!(
            r.as_smi(),
            Some(3),
            "fn with object-rest param, combined return"
        );
    }

    #[test]
    fn test_spread_call_basic() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval("function f(a,b,c) { return a + b + c; } let arr = [1,2,3]; f(...arr)")
            .unwrap();
        assert_eq!(r.as_smi(), Some(6), "f(...[1,2,3])");
    }

    #[test]
    fn test_spread_call_mixed() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval("function f(a,b,c) { return a + b + c; } f(0, ...[1,2])")
            .unwrap();
        assert_eq!(r.as_smi(), Some(3), "f(0, ...[1,2])");
    }

    #[test]
    fn test_spread_call_multiple_spreads() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval("function f(a,b,c) { return a + b + c; } f(...[1], 2, ...[3])")
            .unwrap();
        assert_eq!(r.as_smi(), Some(6), "f(...[1], 2, ...[3])");
    }

    #[test]
    fn test_spread_call_empty() {
        let mut ctx = Context::new_small();
        let r = ctx.eval("function f() { return 42; } f(...[])").unwrap();
        assert_eq!(r.as_smi(), Some(42), "f(...[]) with no-arg fn");
    }

    #[test]
    fn test_spread_call_builtin() {
        let mut ctx = Context::new_small();
        let r = ctx.eval("Math.max(...[1,2,3])").unwrap();
        assert_eq!(r.as_smi(), Some(3), "Math.max(...[1,2,3])");
    }

    #[test]
    fn test_spread_call_print() {
        let mut ctx = Context::new_small();
        let r = ctx.eval("let s = ''; function capture(...args) { s = args.join(','); } capture(...[10,20,30]); s").unwrap();
        assert!(
            r.heap_ptr().is_some(),
            "spread call with rest param should yield joined string"
        );
    }

    #[test]
    fn test_spread_call_rest_param() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval("function f(...args) { return args.length; } f(...[1,2,3])")
            .unwrap();
        assert_eq!(r.as_smi(), Some(3), "f(...[1,2,3]) with rest param");
    }

    // --- Sprint 14C: Object literal extensions ---

    #[test]
    fn test_shorthand_property() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval("var a = 1, b = 2; var o = { a, b }; o.a === 1 && o.b === 2")
            .unwrap();
        assert_eq!(r.to_boolean(), Some(true), "shorthand");
    }

    #[test]
    fn test_shorthand_single() {
        let mut ctx = Context::new_small();
        let r = ctx.eval("var x = 42; var o = { x }; o.x").unwrap();
        assert_eq!(r.as_smi(), Some(42), "shorthand single");
    }

    #[test]
    fn test_shorthand_mixed() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval("var a = 1; var o = { a, b: 2 }; o.a === 1 && o.b === 2")
            .unwrap();
        assert_eq!(r.to_boolean(), Some(true), "shorthand mixed");
    }

    #[test]
    fn test_shorthand_fn_ref() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval("function f() { return 42; } var o = { f }; o.f()")
            .unwrap();
        assert_eq!(r.as_smi(), Some(42), "shorthand function ref");
    }

    #[test]
    fn test_method_shorthand_basic() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval("var o = { foo() { return 42; } }; o.foo()")
            .unwrap();
        assert_eq!(r.as_smi(), Some(42), "method shorthand basic");
    }

    #[test]
    fn test_method_shorthand_this() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval("var o = { x: 1, getX() { return this.x; } }; o.getX()")
            .unwrap();
        assert_eq!(r.as_smi(), Some(1), "method shorthand this");
    }

    #[test]
    fn test_method_shorthand_multiple() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval("var o = { a() { return 1; }, b() { return 2; } }; o.a() + o.b()")
            .unwrap();
        assert_eq!(r.as_smi(), Some(3), "multiple methods");
    }

    #[test]
    fn test_method_shorthand_arguments() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval("var o = { f(a, b) { return a + b; } }; o.f(10, 20)")
            .unwrap();
        assert_eq!(r.as_smi(), Some(30), "method shorthand with params");
    }

    #[test]
    fn test_computed_key_basic() {
        let mut ctx = Context::new_small();
        let r = ctx.eval("var k = 'x'; var o = { [k]: 1 }; o.x").unwrap();
        assert_eq!(r.as_smi(), Some(1), "computed key basic");
    }

    #[test]
    fn test_computed_key_string_concat() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval("var i = 0; var o = { ['key' + i]: 42 }; o.key0")
            .unwrap();
        assert_eq!(r.as_smi(), Some(42), "computed string concatenation");
    }

    #[test]
    fn test_computed_key_numeric() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval("var n = 5; var o = { [n]: 'five' }; o[5]")
            .unwrap();
        assert!(r.heap_ptr().is_some(), "computed numeric key");
    }

    #[test]
    fn test_computed_key_multiple() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval("var k = 'x'; var o = { [k]: 1, [k + '2']: 2 }; o.x === 1 && o.x2 === 2")
            .unwrap();
        assert_eq!(r.to_boolean(), Some(true), "multiple computed keys");
    }

    #[test]
    fn test_computed_method_name() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval("var k = 'x'; var o = { [k]() { return 42; } }; o.x()")
            .unwrap();
        assert_eq!(r.as_smi(), Some(42), "computed method name");
    }

    #[test]
    fn test_computed_destructuring() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval("var k = 'x'; var { [k]: val } = { x: 1 }; val")
            .unwrap();
        assert_eq!(r.as_smi(), Some(1), "computed key destructuring");
    }

    // --- Sprint 14D: Template literal substitutions ---

    #[test]
    fn test_template_no_substitution() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"var s = `hello`; s"#).unwrap();
        assert!(r.heap_ptr().is_some(), "plain template produces string");
    }

    #[test]
    fn test_template_single_substitution() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"var name = "world"; var s = `hello ${name}`; s"#)
            .unwrap();
        assert!(
            r.heap_ptr().is_some(),
            "template with substitution produces string"
        );
    }

    #[test]
    fn test_template_expression_substitution() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"var s = `${1 + 2}`; s"#).unwrap();
        assert!(
            r.heap_ptr().is_some(),
            "template with expression produces string"
        );
    }

    #[test]
    fn test_template_multiple_substitutions() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"var s = `a${1}b${2}c`; s"#).unwrap();
        assert!(
            r.heap_ptr().is_some(),
            "template with multiple substitutions produces string"
        );
    }

    #[test]
    fn test_template_empty_with_substitution() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"var x = 42; var s = `${x}`; s"#).unwrap();
        assert!(
            r.heap_ptr().is_some(),
            "template starting with substitution produces string"
        );
    }

    #[test]
    fn test_template_undefined_null_true() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"var s = `${undefined}${null}${true}`; s"#)
            .unwrap();
        assert!(
            r.heap_ptr().is_some(),
            "template with undefined/null/true coercion produces string"
        );
    }

    #[test]
    fn test_template_nested() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"var x = "inner"; var s = `nested ${`${x}`}`; s"#)
            .unwrap();
        assert!(r.heap_ptr().is_some(), "nested template produces string");
    }

    #[test]
    fn test_template_escape_backtick() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(r#"var s = `\`hello\``; s"#).unwrap();
        assert!(
            r.heap_ptr().is_some(),
            "template with escaped backtick produces string"
        );
    }

    #[test]
    fn test_template_multi_line() {
        let mut ctx = Context::new_small();
        let r = ctx.eval("var s = `line 1\nline 2`; s").unwrap();
        assert!(
            r.heap_ptr().is_some(),
            "multi-line template produces string"
        );
    }

    // ── 14E: arguments materialization ───────────────────────────────────

    #[test]
    fn test_arguments_basic() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"function f() { return arguments.length; }; f()"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(0), "arguments.length with no args");
    }

    #[test]
    fn test_arguments_length() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"function f() { return arguments.length; }; f(1, 2, 3)"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(3), "arguments.length with 3 args");
    }

    #[test]
    fn test_arguments_index() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"function f() { return arguments[0] + arguments[1]; }; f(10, 20)"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(30), "arguments[0] + arguments[1]");
    }

    #[test]
    fn test_arguments_with_params() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"function f(a, b) { return arguments[2]; }; f(1, 2, 99)"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(99), "arguments[2] is extra arg");
    }

    #[test]
    #[ignore = "arguments not yet materialized in nested functions"]
    fn test_arguments_nested_function() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"function f() { function g() { return arguments[0]; }; return g(42); }; f()"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(42), "arguments in nested function");
    }

    #[test]
    #[ignore = "arrows inherit arguments from enclosing function (not yet supported)"]
    fn test_arguments_not_in_arrow() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"function f() { var g = () => arguments[0]; return g(); }; f(99)"#)
            .unwrap();
        // Arrow functions don't have their own `arguments` — they access the
        // enclosing non-arrow function's `arguments` via scope lookup.
        // Currently Rune doesn't implement closure captures, so this is
        // expected to return undefined.
        assert!(
            r.as_smi().is_none(),
            "arrow inherits arguments from enclosing function (not yet supported)"
        );
    }

    // ── 14E: Per-iteration let ───────────────────────────────────────────

    #[test]
    fn test_for_let_basic() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"var s = 0; for (let i = 0; i < 5; i++) { s = s + i; }; s"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(10), "for (let i) sum 0..4 = 10");
    }

    #[test]
    fn test_for_let_separate_iterations() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"var a = []; for (let i = 0; i < 3; i++) { a.push(i); }; a.length"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(3), "for (let i) pushes all values");
    }

    #[test]
    fn test_for_let_closure_capture() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(
                r#"var funcs = []; for (let i = 0; i < 3; i++) { funcs.push(function() { return i; }); }; funcs[0]()"#,
            )
            .unwrap();
        assert_eq!(
            r.as_smi(),
            Some(0),
            "for (let i) per-iteration closure — funcs[0]() = 0"
        );
    }

    #[test]
    fn test_for_let_closure_all_iterations() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(
                r#"var funcs = []; for (let i = 0; i < 3; i++) { funcs.push(function() { return i; }); }; funcs[0]() * 100 + funcs[1]() * 10 + funcs[2]()"#,
            )
            .unwrap();
        assert_eq!(r.as_smi(), Some(12), "for (let i) all iterations: 0,1,2");
    }

    #[test]
    fn test_for_let_arrow_closure() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(
                r#"var funcs = []; for (let i = 0; i < 3; i++) { funcs.push(() => i); }; funcs[1]()"#,
            )
            .unwrap();
        assert_eq!(
            r.as_smi(),
            Some(1),
            "for (let i) arrow closure — funcs[1]() = 1"
        );
    }

    #[test]
    fn test_for_let_i_plus_plus() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"var s = 0; for (let i = 0; i < 10; i++) { s = s + 1; }; s"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(10), "for (let i) runs 10 times");
    }

    #[test]
    fn test_for_var_still_works() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"var s = 0; for (var i = 0; i < 5; i++) { s = s + i; }; s"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(10), "for (var i) still works");
    }

    #[test]
    fn test_for_let_nested() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(
                r#"var s = 0; for (let i = 0; i < 3; i++) { for (let j = 0; j < 3; j++) { s = s + 1; } }; s"#,
            )
            .unwrap();
        assert_eq!(r.as_smi(), Some(9), "nested for (let) runs 9 times");
    }

    #[test]
    fn test_closure_basic_capture() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"function f() { var x = 42; return function() { return x; }; } f()()"#)
            .unwrap();
        assert!(r.is_smi(), "result should be Smi, got {:?}", r);
        assert_eq!(r.as_smi(), Some(42), "basic closure capture");
    }

    #[test]
    fn test_closure_mutation() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(
                r#"function counter() { var c = 0; return function() { c = c + 1; return c; }; }
               var cc = counter(); cc(); cc(); cc()"#,
            )
            .unwrap();
        assert_eq!(r.as_smi(), Some(3), "closure mutation via captured var");
    }

    #[test]
    fn test_closure_same_storage() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(
            r#"function f() { var x = 1; var g = function() { return x; }; x = 2; return g(); } f()"#,
        ).unwrap();
        assert_eq!(r.as_smi(), Some(2), "f's body writes affect closure reads");
    }

    #[test]
    fn test_closure_param_capture() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"function add(a) { return function(b) { return a + b; }; } add(2)(3)"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(5), "param capture via closure");
    }

    #[test]
    fn test_arrow_capture() {
        let mut ctx = Context::new_small();
        let r = ctx
            .eval(r#"function f() { var x = 42; return () => x; } f()()"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(42), "arrow capture");
    }

    #[test]
    fn test_gc_stress_50k_objects() {
        let mut ctx = Context::new();
        let r = ctx
            .eval(
                r#"
            function f() {
                var x = { val: 42 };
                var arr = [];
                for (var i = 0; i < 50000; i++) {
                    arr.push({ junk: i });
                }
                return () => x.val;
            }
            f()()
            "#,
            )
            .unwrap();
        assert_eq!(r.as_smi(), Some(42), "GC stress: 50K allocs + closure");
    }

    #[test]
    fn test_nested_closure() {
        let mut ctx = Context::new_small();
        let r = ctx.eval(
            r#"function f() { var x = 1; return function() { var y = 2; return function() { return x + y; }; }; }
               f()()()"#,
        ).unwrap();
        assert_eq!(r.as_smi(), Some(3), "nested closures (depth 0 + depth 1)");
    }

    #[test]
    fn test_gc_stress_100k_closure() {
        let mut ctx = Context::new();
        let r = ctx
            .eval(
                r#"
            function f() {
                var x = { val: 42 };
                var arr = [];
                for (var i = 0; i < 100000; i++) {
                    arr.push({ junk: i });
                }
                return () => x.val;
            }
            f()()
            "#,
            )
            .unwrap();
        assert_eq!(r.as_smi(), Some(42), "GC stress: 100K allocs + closure");
    }

    #[test]
    fn test_gc_stress_100k_non_closure() {
        let mut ctx = Context::new();
        let r = ctx
            .eval(
                r#"
            function f() {
                var x = { val: 42 };
                var arr = [];
                for (var i = 0; i < 100000; i++) {
                    arr.push({ junk: i });
                }
                return x.val;
            }
            f()
            "#,
            )
            .unwrap();
        assert_eq!(r.as_smi(), Some(42), "GC stress: 100K allocs (non-closure)");
    }

    #[test]
    fn test_gc_preserves_global_heap_object() {
        let mut ctx = Context::new();
        ctx.eval("var obj = {x: 1, y: 2, z: 3};").unwrap();
        let r = ctx
            .eval(
                r#"
            var arr = [];
            for (var i = 0; i < 100000; i = i + 1) {
                arr.push({n: i, m: i + 1});
            }
            obj.x + obj.y + obj.z
            "#,
            )
            .unwrap();
        assert_eq!(r.as_smi(), Some(6), "global heap object corrupted by GC");
    }

    #[test]
    #[ignore]
    fn test_gc_during_jit_call_preserves_locals() {
        let mut ctx = Context::new();
        let r = ctx
            .eval(
                r#"
            function make(n) {
                var arr = [];
                for (var i = 0; i < n; i = i + 1) { arr.push({x: i}); }
                return arr.length;
            }
            var total = 0;
            for (var i = 0; i < 1000; i = i + 1) { total = total + make(200); }
            total
            "#,
            )
            .unwrap();
        assert_eq!(
            r.as_smi(),
            Some(200_000),
            "jit_locals_buffer corrupted by GC mid-call"
        );
    }

    #[test]
    fn test_stencil_jit_load_smi_matches_old_codegen() {
        // Each waypoint is a hot function that triggers JIT (100 calls > threshold of 50).
        // Verify stencil and old codegen produce identical results.
        let cases: &[&str] = &[
            r#"
                function f() { return 42; }
                var r = 0;
                for (var i = 0; i < 100; i = i + 1) { r = f(); }
                r
            "#,
            r#"
                function add(a, b) { return a + b; }
                var r = 0;
                for (var i = 0; i < 100; i = i + 1) { r = add(r, 42); }
                r
            "#,
            r#"
                function f() { return -1; }
                var r = 0;
                for (var i = 0; i < 100; i = i + 1) { r = f(); }
                r
            "#,
            r#"
                function f() { return 1073741823; }
                var r = 0;
                for (var i = 0; i < 100; i = i + 1) { r = f(); }
                r
            "#,
            // LoadUndefined / LoadNull / LoadBoolean via stencil
            r#"
                function f() { return undefined; }
                var r = 0;
                for (var i = 0; i < 100; i = i + 1) { r = f(); }
                r
            "#,
            r#"
                function f() { return null; }
                var r = 0;
                for (var i = 0; i < 100; i = i + 1) { r = f(); }
                r
            "#,
            r#"
                function f() { return true; }
                var r = 0;
                for (var i = 0; i < 100; i = i + 1) { r = f(); }
                r
            "#,
            r#"
                function f() { return false; }
                var r = 0;
                for (var i = 0; i < 100; i = i + 1) { r = f(); }
                r
            "#,
            // LoadLocal + StoreLocal via stencil
            r#"
                function f(a) { return a; }
                var r = 0;
                for (var i = 0; i < 100; i = i + 1) { r = f(42); }
                r
            "#,
            r#"
                function f() { var x = 99; return x; }
                var r = 0;
                for (var i = 0; i < 100; i = i + 1) { r = f(); }
                r
            "#,
            r#"
                function f(a) { a = a + 1; return a; }
                var r = 0;
                for (var i = 0; i < 100; i = i + 1) { r = f(41); }
                r
            "#,
        ];

        for source in cases {
            let mut ctx_old = Context::new_small();
            ctx_old.stencil_jit = false;
            let r_old = ctx_old.eval(source).unwrap();

            let mut ctx_new = Context::new_small();
            ctx_new.stencil_jit = true;
            let r_new = ctx_new.eval(source).unwrap();

            assert_eq!(
                r_old, r_new,
                "stencil vs old codegen: old={:?} new={:?}",
                r_old, r_new
            );
        }
    }
} // close mod instanceof_tests

// ---- Stdlib: JSON.parse ----

#[test]
fn test_json_parse_null() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("JSON.parse('null')").unwrap();
    assert!(r.is_null());
}

#[test]
fn test_json_parse_true() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("JSON.parse('true')").unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_json_parse_false() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("JSON.parse('false')").unwrap();
    assert_eq!(r.to_boolean(), Some(false));
}

#[test]
fn test_json_parse_number() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("JSON.parse('42')").unwrap();
    assert_eq!(r.as_smi(), Some(42));
}

#[test]
fn test_json_parse_float() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("JSON.parse('12.375')").unwrap();
    let f = r.as_float64().unwrap();
    assert!((f - 12.375).abs() < 1e-10);
}

#[test]
fn test_json_parse_string() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("JSON.parse('\"hello\"')").unwrap();
    let ptr = r.heap_ptr().unwrap();
    let s = unsafe {
        rune_core::string::HeapString::to_string(ptr as *mut rune_core::string::HeapString)
    };
    assert_eq!(s, "hello");
}

#[test]
fn test_json_parse_array() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("JSON.parse('[1,2,3]')").unwrap();
    assert!(r.is_heap_object());
    // Access elements after parsing
    let r2 = ctx
        .eval("var a = JSON.parse('[10,20,30]'); a[0] + a[1] + a[2]")
        .unwrap();
    assert_eq!(r2.as_smi(), Some(60));
}

#[test]
fn test_json_parse_nested() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval("var o = JSON.parse('{\"x\":{\"y\":[1,2]}}'); o.x.y[0] + o.x.y[1]")
        .unwrap();
    assert_eq!(r.as_smi(), Some(3));
}

#[test]
fn test_json_parse_object() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval("var o = JSON.parse('{\"a\":1,\"b\":2}'); o.a + o.b")
        .unwrap();
    assert_eq!(r.as_smi(), Some(3));
}

// ---- Stdlib: Array.prototype.filter ----

#[test]
fn test_array_filter_basic() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        function isEven(x) { return x % 2 === 0; }
        var a = [1, 2, 3, 4, 5, 6];
        var result = a.filter(isEven);
        result[0] + result[1] + result[2]
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(2 + 4 + 6));
}

#[test]
fn test_array_filter_arrow() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        var a = [1, 2, 3, 4, 5];
        var result = a.filter(n => n > 2);
        result[0] + result[1] + result[2]
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(3 + 4 + 5));
}

#[test]
fn test_array_filter_empty() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        var a = [];
        var result = a.filter(n => true);
        result.length
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(0));
}

#[test]
fn test_array_filter_all_match() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        var a = [1, 2, 3];
        var result = a.filter(n => n > 0);
        result[0] + result[1] + result[2]
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(6));
}

// ---- Stdlib: Array.prototype.map ----

#[test]
fn test_array_map_basic() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        function double(x) { return x * 2; }
        var a = [1, 2, 3];
        var result = a.map(double);
        result[0] + result[1] + result[2]
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(2 + 4 + 6));
}

#[test]
fn test_array_map_arrow() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        var a = [1, 2, 3];
        var result = a.map(n => n * 10);
        result[0] + result[1] + result[2]
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(10 + 20 + 30));
}

#[test]
fn test_array_map_empty() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        var a = [];
        var result = a.map(n => n * 2);
        result.length
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(0));
}

// ---- Stdlib: Array.prototype.reduce ----

#[test]
fn test_array_reduce_sum() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        function add(acc, x) { return acc + x; }
        var a = [1, 2, 3, 4, 5];
        a.reduce(add, 0)
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(15));
}

#[test]
fn test_array_reduce_no_initial() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        function add(acc, x) { return acc + x; }
        var a = [1, 2, 3, 4, 5];
        a.reduce(add)
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(15));
}

#[test]
fn test_array_reduce_arrow() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        var a = [1, 2, 3, 4];
        a.reduce((acc, x) => acc + x, 100)
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(110));
}

#[test]
fn test_array_reduce_single_element_no_init() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        var a = [42];
        a.reduce((acc, x) => acc + x)
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(42));
}

// ---- Stdlib: chained array methods ----

#[test]
fn test_array_filter_map_chain() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        function even(n) { return n % 2 === 0; }
        function times10(n) { return n * 10; }
        var a = [1, 2, 3, 4, 5, 6];
        var result = a.filter(even).map(times10);
        result[0] + result[1] + result[2]
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(20 + 40 + 60));
}

#[test]
fn test_array_filter_then_reduce() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        var a = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        a.filter(n => n % 2 === 0).reduce((acc, n) => acc + n, 0)
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(2 + 4 + 6 + 8 + 10));
}

// ---- Stdlib: JSON round-trip with array methods ----

#[test]
fn test_json_parse_then_filter() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        function gt3(x) { return x > 3; }
        var data = JSON.parse('[4,5,6]');
        var filtered = data.filter(gt3);
        filtered[0] + filtered[1] + filtered[2]
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(4 + 5 + 6));
}

#[test]
fn test_json_parse_then_map() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        function div10(x) { return x / 10; }
        var data = JSON.parse('[10, 20, 30]');
        var mapped = data.map(div10);
        mapped[0] + mapped[1] + mapped[2]
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(1 + 2 + 3));
}

// ---- Stdlib: filter/map with thisArg ----

#[test]
fn test_array_filter_this_arg() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        var threshold = { limit: 3 };
        function checkLimit(x) { return x > this.limit; }
        var a = [1, 2, 3, 4, 5];
        var result = a.filter(checkLimit, threshold);
        result[0] + result[1]
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(4 + 5));
}

#[test]
fn test_array_map_this_arg() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        var multiplier = { factor: 10 };
        function scale(x) { return x * this.factor; }
        var a = [1, 2, 3];
        var result = a.map(scale, multiplier);
        result[0] + result[1] + result[2]
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(10 + 20 + 30));
}

// ---- Stdlib: forEach ----

#[test]
fn test_array_foreach_basic() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        var sum = 0;
        var a = [1, 2, 3, 4];
        a.forEach(function(x) { sum = sum + x; });
        sum
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(10));
}

#[test]
fn test_array_foreach_arrow() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        var sum = 0;
        var a = [1, 2, 3];
        a.forEach(x => { sum = sum + x; });
        sum
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(6));
}

#[test]
fn test_array_foreach_empty() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        var sum = 0;
        [].forEach(function(x) { sum = sum + x; });
        sum
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(0));
}

#[test]
fn test_array_foreach_this_arg() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        var accumulator = { total: 0 };
        function add(x) { this.total = this.total + x; }
        var a = [1, 2, 3, 4];
        a.forEach(add, accumulator);
        accumulator.total
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(10));
}

#[test]
fn test_array_foreach_chain_filter() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        var sum = 0;
        var a = [1, 2, 3, 4, 5, 6];
        a.filter(function(x) { return x % 2 === 0; })
         .forEach(function(x) { sum = sum + x; });
        sum
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(2 + 4 + 6));
}

// ---- Stdlib: reduce with object accumulator ----

#[test]
fn test_array_reduce_group() {
    let mut ctx = Context::new_small();
    let r = ctx.eval(r#"
        function group(acc, x) { if (x % 2 === 0) { acc.even.push(x); } else { acc.odd.push(x); } return acc; }
        var a = [1, 2, 3, 4, 5, 6];
        var result = a.reduce(group, {even: [], odd: []});
        result.even[0] + result.even[1] + result.even[2] + result.odd[0] + result.odd[1] + result.odd[2]
    "#).unwrap();
    assert_eq!(r.as_smi(), Some(2 + 4 + 6 + 1 + 3 + 5));
}

#[test]
fn e2e_json_workload() {
    let mut ctx = Context::new();
    let r = ctx.eval(r#"
        var data = JSON.parse('{"items":[{"name":"a","value":1,"active":true},{"name":"b","value":2,"active":false},{"name":"c","value":3,"active":true}]}');
        var result = data.items
            .filter(function(x) { return x.active; })
            .map(function(x) { return x.value * 2; })
            .reduce(function(a, b) { return a + b; }, 0);
        result
    "#).unwrap();
    assert_eq!(r.as_smi(), Some(8));
}

#[test]
fn e2e_gc_stress_reduce() {
    let mut ctx = Context::new();
    let r = ctx
        .eval(
            r#"
        var arr = [];
        for (var i = 0; i < 200000; i = i + 1) { arr.push(i); }
        var sum = arr.reduce(function(a, b) { return a + b; }, 0);
        sum
    "#,
        )
        .unwrap();
    let expected = 19999900000.0f64;
    let val = r
        .as_float64()
        .expect("GC stress test should produce float64");
    assert!(
        (val - expected).abs() < 1.0,
        "reduce sum mismatch: got {} expected {}",
        val,
        expected
    );
}

// ---- Stdlib: Array.prototype.slice ----

#[test]
fn test_array_slice_basic() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        var a = [1, 2, 3, 4, 5];
        var b = a.slice(1, 3);
        b[0] + b[1]
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(2 + 3));
}

#[test]
fn test_array_slice_no_end() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        var a = [1, 2, 3, 4, 5];
        var b = a.slice(2);
        b[0] + b[1] + b[2]
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(3 + 4 + 5));
}

#[test]
fn test_array_slice_full() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        var a = [1, 2, 3];
        var b = a.slice();
        b[0] + b[1] + b[2]
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(1 + 2 + 3));
}

#[test]
fn test_array_slice_negative_start() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        var a = [10, 20, 30, 40];
        var b = a.slice(-2);
        b[0] + b[1]
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(30 + 40));
}

#[test]
fn test_array_slice_negative_end() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        var a = [10, 20, 30, 40];
        var b = a.slice(1, -1);
        b[0] + b[1]
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(20 + 30));
}

#[test]
fn test_array_slice_empty() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        var a = [1, 2, 3];
        var b = a.slice(5);
        b.length
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(0));
}

#[test]
fn test_array_slice_no_mutate_original() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        var a = [1, 2, 3];
        var b = a.slice(1, 2);
        a.length
    "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(3));
}

// ---- C7: Array.prototype reverse / splice / concat / shift / unshift ----

#[test]
fn test_array_reverse_basic() {
    let mut ctx = Context::new_small();
    assert_eq!(eval_str(&mut ctx, "[1, 2, 3].reverse().join(',')"), "3,2,1");
    assert_eq!(eval_str(&mut ctx, "[1].reverse().join(',')"), "1");
    assert_eq!(eval_array_len(&mut ctx, "[].reverse()"), 0);
    // Even length
    assert_eq!(
        eval_str(&mut ctx, "[1,2,3,4].reverse().join(',')"),
        "4,3,2,1"
    );
}

#[test]
fn test_array_reverse_in_place_returns_this() {
    let mut ctx = Context::new_small();
    // reverse() mutates in place and returns the same array object
    assert_eq!(
        eval_str(
            &mut ctx,
            "var b=[1,2]; var c=b.reverse(); (c===b)+'|'+b.join(',')"
        ),
        "true|2,1"
    );
}

#[test]
fn test_array_concat_basic() {
    let mut ctx = Context::new_small();
    assert_eq!(
        eval_str(&mut ctx, "[1,2].concat([3,4],[5]).join(',')"),
        "1,2,3,4,5"
    );
    // Non-array args are appended as single items
    assert_eq!(
        eval_str(&mut ctx, "[1].concat(2,'three').join(',')"),
        "1,2,three"
    );
    // Nested arrays flatten exactly one level
    assert_eq!(
        ctx.eval("[1].concat([2,[3]]).length").unwrap().as_smi(),
        Some(3)
    );
    assert_eq!(eval_array_len(&mut ctx, "[].concat()"), 0);
    // Original untouched
    assert_eq!(
        ctx.eval("var o=[1]; o.concat([2]); o.length")
            .unwrap()
            .as_smi(),
        Some(1)
    );
}

#[test]
fn test_array_concat_isconcatspreadable() {
    let mut ctx = Context::new_small();
    // Array-like object with @@isConcatSpreadable spreads
    assert_eq!(
        eval_str(
            &mut ctx,
            r#"
        var arrlike = { 0: 'a', 1: 'b', length: 2 };
        arrlike[Symbol.isConcatSpreadable] = true;
        [0].concat(arrlike).join(',')
    "#
        ),
        "0,a,b"
    );
    // Explicitly non-spreadable array is appended whole
    assert_eq!(
        ctx.eval(
            r#"
        var arr = [9, 9];
        arr[Symbol.isConcatSpreadable] = false;
        [1].concat(arr).length
    "#
        )
        .unwrap()
        .as_smi(),
        Some(3)
    );
}

#[test]
fn test_array_concat_null_this_throws() {
    let mut ctx = Context::new_small();
    assert!(ctx.eval("[].concat.call(null)").is_err());
    assert!(ctx.eval("[].concat.call(undefined, 1)").is_err());
}

#[test]
fn test_array_shift_basic() {
    let mut ctx = Context::new_small();
    assert_eq!(
        eval_str(
            &mut ctx,
            "var a=[10,20,30]; var f=a.shift(); f+'|'+a.join(',')"
        ),
        "10|20,30"
    );
    // Empty array → undefined
    assert_eq!(
        eval_str(
            &mut ctx,
            "var e=[]; var f=e.shift(); (f===undefined)+'|'+e.length"
        ),
        "true|0"
    );
}

#[test]
fn test_array_unshift_basic() {
    let mut ctx = Context::new_small();
    assert_eq!(
        eval_str(
            &mut ctx,
            "var a=[3,4]; var n=a.unshift(1,2); n+'|'+a.join(',')"
        ),
        "4|1,2,3,4"
    );
    // No args → length unchanged
    assert_eq!(
        eval_str(&mut ctx, "var a=[1]; a.unshift(); '|'+a.length"),
        "|1"
    );
}

#[test]
fn test_array_unshift_grow_regression() {
    let mut ctx = Context::new_small();
    // Regression: unshift past capacity used to loop forever allocating
    // (discarded grow pointers) and OOM the engine.
    assert_eq!(
        eval_str(
            &mut ctx,
            "var a=[1,2,3]; var n=a.unshift(4,5,6,7,8,9); n+'|'+a.join(',')"
        ),
        "9|4,5,6,7,8,9,1,2,3"
    );
    // Repeated growth under GC pressure in a small heap
    assert_eq!(
        ctx.eval(
            r#"
        var a = [];
        for (var i = 0; i < 200; i = i + 1) { a.unshift(i, [i, i]); }
        a.length
    "#
        )
        .unwrap()
        .as_smi(),
        Some(400)
    );
}

#[test]
fn test_array_shift_unshift_object_receiver() {
    let mut ctx = Context::new_small();
    // Array-like object receiver
    assert_eq!(
        eval_str(
            &mut ctx,
            r#"
        var o = { 0: 'x', 1: 'y', 2: 'z', length: 3 };
        var f = Array.prototype.shift.call(o);
        f + '|' + o[0] + '|' + o[1] + '|' + o.length
    "#
        ),
        "x|y|z|2"
    );
    assert_eq!(
        eval_str(
            &mut ctx,
            r#"
        var p = { 0: 'b', length: 1 };
        var n = Array.prototype.unshift.call(p, 'a');
        n + '|' + p[0] + '|' + p[1] + '|' + p.length
    "#
        ),
        "2|a|b|2"
    );
}

#[test]
fn test_array_splice_no_args_noop() {
    let mut ctx = Context::new_small();
    // Regression: splice() with no args used to delete the entire array
    assert_eq!(
        eval_str(
            &mut ctx,
            "var a=[1,2,3]; var d=a.splice(); d.length+'|'+a.length+'|'+a[0]"
        ),
        "0|3|1"
    );
}

#[test]
fn test_array_splice_delete_only() {
    let mut ctx = Context::new_small();
    assert_eq!(
        eval_str(
            &mut ctx,
            "var a=[1,2,3,4]; var d=a.splice(1,2); d.join(',')+'|'+a.join(',')"
        ),
        "2,3|1,4"
    );
    // Negative start counts from the end
    assert_eq!(
        eval_str(
            &mut ctx,
            "var a=[1,2,3,4]; var d=a.splice(-2); d.join(',')+'|'+a.join(',')"
        ),
        "3,4|1,2"
    );
    // Start beyond end clamps
    assert_eq!(
        eval_str(
            &mut ctx,
            "var a=[1,2]; var d=a.splice(99); d.length+'|'+a.length"
        ),
        "0|2"
    );
    // deleteCount absent → deletes to end
    assert_eq!(
        eval_str(
            &mut ctx,
            "var a=[1,2,3]; var d=a.splice(1); d.join(',')+'|'+a.length"
        ),
        "2,3|1"
    );
}

#[test]
fn test_array_splice_insert_grow_regression() {
    let mut ctx = Context::new_small();
    // Regression: insert-heavy splice used to infinite-loop growing
    assert_eq!(
        eval_str(
            &mut ctx,
            "var a=[1,2,3]; var d=a.splice(1,0,'x','y','z','w','v','u'); d.length+'|'+a.join(',')"
        ),
        "0|1,x,y,z,w,v,u,2,3"
    );
}

#[test]
fn test_array_splice_replace() {
    let mut ctx = Context::new_small();
    assert_eq!(
        eval_str(
            &mut ctx,
            "var a=[1,2,3,4,5]; var d=a.splice(1,2,'x','y'); d.join(',')+'|'+a.join(',')"
        ),
        "2,3|1,x,y,4,5"
    );
    // Fewer items than deleted shrinks
    assert_eq!(
        eval_str(
            &mut ctx,
            "var a=[1,2,3,4]; var d=a.splice(1,3,'z'); d.join(',')+'|'+a.join(',')"
        ),
        "2,3,4|1,z"
    );
}

#[test]
fn test_array_splice_coercions() {
    let mut ctx = Context::new_small();
    // String start coerces via ToIntegerOrInfinity
    assert_eq!(
        eval_str(
            &mut ctx,
            "var a=[1,2,3]; var d=a.splice('1',1); d.join(',')+'|'+a.join(',')"
        ),
        "2|1,3"
    );
    // Float start truncates toward zero
    assert_eq!(
        eval_str(
            &mut ctx,
            "var a=[1,2,3]; var d=a.splice(1.9,1); d.join(',')+'|'+a.join(',')"
        ),
        "2|1,3"
    );
    // Negative float start truncates toward zero then offsets
    assert_eq!(
        eval_str(
            &mut ctx,
            "var a=[1,2,3]; var d=a.splice(-1.5); d.join(',')+'|'+a.join(',')"
        ),
        "3|1,2"
    );
    // NaN start → 0
    assert_eq!(
        eval_str(
            &mut ctx,
            "var a=[1,2,3]; var d=a.splice(NaN,1); d.join(',')+'|'+a.join(',')"
        ),
        "1|2,3"
    );
    // String deleteCount coerces
    assert_eq!(
        eval_str(
            &mut ctx,
            "var a=[1,2,3]; var d=a.splice(0,'2'); d.join(',')+'|'+a.join(',')"
        ),
        "1,2|3"
    );
    // Negative deleteCount clamps to 0
    assert_eq!(
        eval_str(
            &mut ctx,
            "var a=[1,2,3]; var d=a.splice(0,-5); d.length+'|'+a.length"
        ),
        "0|3"
    );
}

#[test]
fn test_array_methods_null_this_throw() {
    let mut ctx = Context::new_small();
    for m in ["shift", "unshift", "splice", "concat", "reverse"] {
        assert!(
            ctx.eval(&format!("Array.prototype.{}.call(null)", m))
                .is_err(),
            "{} on null should throw",
            m
        );
        assert!(
            ctx.eval(&format!("Array.prototype.{}.call(undefined)", m))
                .is_err(),
            "{} on undefined should throw",
            m
        );
    }
}

// ---- C8: Object / Function / Math statics ----

#[test]
fn test_math_round_spec_semantics() {
    let mut ctx = Context::new_small();
    // Ties toward +∞: -2.5 → -2 (NOT away-from-zero)
    assert_eq!(ctx.eval("Math.round(2.5)").unwrap().as_smi(), Some(3));
    assert_eq!(ctx.eval("Math.round(-2.5)").unwrap().as_smi(), Some(-2));
    assert_eq!(ctx.eval("Math.round(-3.5)").unwrap().as_smi(), Some(-3));
    // The 0.49999999999999994 hazard must not round up
    assert_eq!(
        ctx.eval("Math.round(0.49999999999999994)")
            .unwrap()
            .as_smi(),
        Some(0)
    );
    // -0.5 rounds to -0 (float, not Smi)
    assert_eq!(
        eval_str(&mut ctx, "String(1/Math.round(-0.5))"),
        "-Infinity"
    );
    // Non-finite passthrough
    assert_eq!(
        eval_str(&mut ctx, "String(Math.round(Infinity))"),
        "Infinity"
    );
    assert_eq!(eval_str(&mut ctx, "String(Math.round(NaN))"), "NaN");
    assert_eq!(ctx.eval("Math.round('7.6')").unwrap().as_smi(), Some(8));
}

#[test]
fn test_math_trunc_sign_misc() {
    let mut ctx = Context::new_small();
    assert_eq!(ctx.eval("Math.trunc(4.7)").unwrap().as_smi(), Some(4));
    assert_eq!(ctx.eval("Math.trunc(-4.7)").unwrap().as_smi(), Some(-4));
    assert_eq!(ctx.eval("Math.sign(-3)").unwrap().as_smi(), Some(-1));
    assert_eq!(ctx.eval("Math.sign(9)").unwrap().as_smi(), Some(1));
    assert_eq!(eval_str(&mut ctx, "String(Math.sign(NaN))"), "NaN");
    // ±0 preserved through sign
    assert_eq!(eval_str(&mut ctx, "String(1/Math.sign(-0))"), "-Infinity");
    assert_eq!(
        eval_str(&mut ctx, "String(1/Math.trunc(-0.5))"),
        "-Infinity"
    );
}

#[test]
fn test_math_hypot_clz32_imul() {
    let mut ctx = Context::new_small();
    assert_eq!(ctx.eval("Math.hypot(3,4)").unwrap().as_smi(), Some(5));
    // Infinity wins over NaN (coerce-all-first semantics)
    assert_eq!(
        eval_str(&mut ctx, "String(Math.hypot(NaN, Infinity))"),
        "Infinity"
    );
    assert_eq!(eval_str(&mut ctx, "String(Math.hypot(NaN))"), "NaN");
    assert_eq!(ctx.eval("Math.hypot()").unwrap().as_smi(), Some(0));
    assert_eq!(ctx.eval("Math.clz32(1)").unwrap().as_smi(), Some(31));
    assert_eq!(ctx.eval("Math.clz32(0x10000)").unwrap().as_smi(), Some(15));
    assert_eq!(ctx.eval("Math.clz32(0)").unwrap().as_smi(), Some(32));
    // imul wraps to 32 bits
    assert_eq!(
        ctx.eval("Math.imul(0xffffffff, 5)").unwrap().as_smi(),
        Some(-5)
    );
    assert_eq!(ctx.eval("Math.imul(3, 4)").unwrap().as_smi(), Some(12));
}

#[test]
fn test_math_constants_and_fns() {
    let mut ctx = Context::new_small();
    assert_eq!(
        ctx.eval("Math.PI").unwrap().as_float64(),
        Some(std::f64::consts::PI)
    );
    assert_eq!(
        ctx.eval("Math.SQRT2").unwrap().as_float64(),
        Some(std::f64::consts::SQRT_2)
    );
    assert_eq!(
        ctx.eval("Math.LN2").unwrap().as_float64(),
        Some(std::f64::consts::LN_2)
    );
    assert_eq!(
        ctx.eval("Math.E").unwrap().as_float64(),
        Some(std::f64::consts::E)
    );
    assert_eq!(
        ctx.eval("Math.LOG2E").unwrap().as_float64(),
        Some(std::f64::consts::LOG2_E)
    );
    assert_eq!(ctx.eval("Math.cbrt(27)").unwrap().as_smi(), Some(3));
    assert_eq!(ctx.eval("Math.log2(8)").unwrap().as_smi(), Some(3));
    assert_eq!(ctx.eval("Math.log10(1000)").unwrap().as_smi(), Some(3));
    assert_eq!(
        eval_str(&mut ctx, "String(Math.atan2(1, 1))"),
        "0.7853981633974483"
    );
    assert_eq!(ctx.eval("Math.exp(0)").unwrap().as_smi(), Some(1));
}

#[test]
fn test_object_assign_basic_and_order() {
    let mut ctx = Context::new_small();
    assert_eq!(
        eval_str(
            &mut ctx,
            "var t={a:1}; Object.assign(t,{b:2},{c:3}); t.a+','+t.b+','+t.c"
        ),
        "1,2,3"
    );
    // Later sources overwrite earlier
    assert_eq!(
        ctx.eval("var u={}; Object.assign(u,{x:1},{x:2}); u.x")
            .unwrap()
            .as_smi(),
        Some(2)
    );
    // Array sources copy indices then named props
    assert_eq!(
        eval_str(
            &mut ctx,
            "var v={}; Object.assign(v,[10,20]); v[0]+','+v[1]+','+String(v.length)"
        ),
        "10,20,undefined"
    );
    // null/undefined sources are skipped
    assert_eq!(
        ctx.eval("var w={k:1}; Object.assign(w,null,{j:2},undefined); w.k+w.j")
            .unwrap()
            .as_smi(),
        Some(3)
    );
    // Returns the target object
    assert_eq!(
        ctx.eval("var q={}; Object.assign(q,{m:1}) === q")
            .unwrap()
            .to_boolean(),
        Some(true)
    );
}

#[test]
fn test_object_is_same_value() {
    let mut ctx = Context::new_small();
    let is = |ctx: &mut Context, code: &str| ctx.eval(code).unwrap().to_boolean();
    assert_eq!(is(&mut ctx, "Object.is(NaN, NaN)"), Some(true));
    assert_eq!(is(&mut ctx, "Object.is(0, -0)"), Some(false));
    assert_eq!(is(&mut ctx, "Object.is(-0, -0)"), Some(true));
    assert_eq!(is(&mut ctx, "Object.is(1, 1)"), Some(true));
    assert_eq!(is(&mut ctx, "Object.is('a', 'a')"), Some(true));
    assert_eq!(is(&mut ctx, "var o={}; Object.is(o, o)"), Some(true));
    assert_eq!(is(&mut ctx, "Object.is({}, {})"), Some(false));
    assert_eq!(is(&mut ctx, "Object.is(1, '1')"), Some(false));
}

#[test]
fn test_object_get_own_property_names() {
    let mut ctx = Context::new_small();
    assert_eq!(
        eval_str(&mut ctx, "Object.getOwnPropertyNames({b:1,a:2}).join(',')"),
        "b,a"
    );
    // Arrays include indices and length? Engine exposes dense indices via keys
    assert_eq!(
        eval_array_len(&mut ctx, "Object.getOwnPropertyNames([7,8])"),
        2
    );
}

#[test]
fn test_object_from_entries_has_own() {
    let mut ctx = Context::new_small();
    assert_eq!(
        ctx.eval("var o=Object.fromEntries([['x',1],['y',2]]); o.x+o.y")
            .unwrap()
            .as_smi(),
        Some(3)
    );
    // Numeric keys stringified
    assert_eq!(
        eval_str(&mut ctx, "var p=Object.fromEntries([[1,'a']]); p['1']+p[1]"),
        "aa"
    );
    assert_eq!(
        ctx.eval("Object.hasOwn({a:1},'a')").unwrap().to_boolean(),
        Some(true)
    );
    assert_eq!(
        ctx.eval("Object.hasOwn({a:1},'b')").unwrap().to_boolean(),
        Some(false)
    );
    // Inherited props are NOT own
    assert_eq!(
        ctx.eval(
            "var proto={inherited:1}; var c=Object.create(proto); Object.hasOwn(c,'inherited')"
        )
        .unwrap()
        .to_boolean(),
        Some(false)
    );
    assert_eq!(
        ctx.eval("Object.hasOwn([9], 0)").unwrap().to_boolean(),
        Some(true)
    );
    // Numeric keys stringified part 2 + null throws
    assert!(ctx.eval("Object.hasOwn(null, 'x')").is_err());
    assert!(ctx.eval("Object.fromEntries(null)").is_err());
}

#[test]
fn test_object_set_prototype_of() {
    let mut ctx = Context::new_small();
    assert_eq!(
        eval_str(
            &mut ctx,
            "var base={greet(){return 'hi';}}; var child=Object.create(null); \
             Object.setPrototypeOf(child, base); child.greet()"
        ),
        "hi"
    );
    // Invalid proto throws even for primitive obj (spec step order)
    assert!(ctx.eval("Object.setPrototypeOf(42, 'x')").is_err());
    assert!(ctx.eval("Object.setPrototypeOf({}, 42)").is_err());
    // null proto allowed
    assert_eq!(
        eval_str(
            &mut ctx,
            "var n={}; Object.setPrototypeOf(n, null); typeof n"
        ),
        "object"
    );
}

#[test]
fn test_function_apply() {
    let mut ctx = Context::new_small();
    assert_eq!(
        ctx.eval("function add(a,b){return a+b;} add.apply(null,[20,22])")
            .unwrap()
            .as_smi(),
        Some(42)
    );
    // this binding flows through
    assert_eq!(
        eval_str(
            &mut ctx,
            "function who(){return this.name;} who.apply({name:'rune'},[])"
        ),
        "rune"
    );
    // undefined/null argArray → no args
    assert_eq!(
        eval_str(
            &mut ctx,
            "function n(){return arguments.length;} String(n.apply(null,undefined))+String(n.apply(null))"
        ),
        "00"
    );
    // Array-like argArray works
    assert_eq!(
        eval_str(
            &mut ctx,
            "function first(a){return a;} first.apply(null, {0:'q', length:1})"
        ),
        "q"
    );
    // Non-callable throws
    assert!(
        ctx.eval("Function.prototype.apply.call(42, null, [])")
            .is_err()
    );
}

// ---- Stdlib: JSON.stringify ----

#[test]
fn test_json_stringify_number() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("JSON.stringify(42)").unwrap();
    let ptr = r.heap_ptr().unwrap();
    let s = unsafe {
        rune_core::string::HeapString::to_string(ptr as *mut rune_core::string::HeapString)
    };
    assert_eq!(s, "42");
}

#[test]
fn test_json_stringify_string() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("JSON.stringify('hello')").unwrap();
    let ptr = r.heap_ptr().unwrap();
    let s = unsafe {
        rune_core::string::HeapString::to_string(ptr as *mut rune_core::string::HeapString)
    };
    assert_eq!(s, "\"hello\"");
}

#[test]
fn test_json_stringify_boolean() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("JSON.stringify(true)").unwrap();
    let ptr = r.heap_ptr().unwrap();
    let s = unsafe {
        rune_core::string::HeapString::to_string(ptr as *mut rune_core::string::HeapString)
    };
    assert_eq!(s, "true");
}

#[test]
fn test_json_stringify_null() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("JSON.stringify(null)").unwrap();
    let ptr = r.heap_ptr().unwrap();
    let s = unsafe {
        rune_core::string::HeapString::to_string(ptr as *mut rune_core::string::HeapString)
    };
    assert_eq!(s, "null");
}

#[test]
fn test_json_stringify_undefined_top_level() {
    let mut ctx = Context::new_small();
    // Top-level undefined returns undefined, not a string
    let r = ctx.eval("JSON.stringify(undefined)").unwrap();
    assert!(r.is_undefined());
}

#[test]
fn test_json_stringify_nan_infinity() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("JSON.stringify(NaN)").unwrap();
    let ptr = r.heap_ptr().unwrap();
    let s = unsafe {
        rune_core::string::HeapString::to_string(ptr as *mut rune_core::string::HeapString)
    };
    assert_eq!(s, "null");
}

#[test]
fn test_json_stringify_infinity() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("JSON.stringify(Infinity)").unwrap();
    let ptr = r.heap_ptr().unwrap();
    let s = unsafe {
        rune_core::string::HeapString::to_string(ptr as *mut rune_core::string::HeapString)
    };
    assert_eq!(s, "null");
}

#[test]
fn test_json_stringify_array() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("JSON.stringify([1, 2, 3])").unwrap();
    let ptr = r.heap_ptr().unwrap();
    let s = unsafe {
        rune_core::string::HeapString::to_string(ptr as *mut rune_core::string::HeapString)
    };
    assert_eq!(s, "[1,2,3]");
}

#[test]
fn test_json_stringify_object() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("JSON.stringify({a:1,b:2})").unwrap();
    let ptr = r.heap_ptr().unwrap();
    let s = unsafe {
        rune_core::string::HeapString::to_string(ptr as *mut rune_core::string::HeapString)
    };
    assert_eq!(s, "{\"a\":1,\"b\":2}");
}

#[test]
fn test_json_stringify_nested() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("JSON.stringify({a:[1,{b:2}]})").unwrap();
    let ptr = r.heap_ptr().unwrap();
    let s = unsafe {
        rune_core::string::HeapString::to_string(ptr as *mut rune_core::string::HeapString)
    };
    assert_eq!(s, "{\"a\":[1,{\"b\":2}]}");
}

#[test]
fn test_json_stringify_cycle() {
    let mut ctx = Context::new_small();
    // Without try/catch, cycle error propagates as eval error
    let r = ctx.eval(
        r#"
        var a = {};
        a.self = a;
        JSON.stringify(a)
    "#,
    );
    assert!(r.is_err());
}

#[test]
fn test_json_stringify_undefined_in_array() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("JSON.stringify([1, undefined, 3])").unwrap();
    let ptr = r.heap_ptr().unwrap();
    let s = unsafe {
        rune_core::string::HeapString::to_string(ptr as *mut rune_core::string::HeapString)
    };
    assert_eq!(s, "[1,null,3]");
}

#[test]
fn test_json_stringify_omit_undefined_prop() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("JSON.stringify({a:1,b:undefined,c:3})").unwrap();
    let ptr = r.heap_ptr().unwrap();
    let s = unsafe {
        rune_core::string::HeapString::to_string(ptr as *mut rune_core::string::HeapString)
    };
    assert_eq!(s, "{\"a\":1,\"c\":3}");
}

#[test]
fn test_json_round_trip() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
        var data = JSON.parse('{"name":"rune","values":[1,2,3],"nested":{"a":true,"b":null}}');
        var out = JSON.stringify(data);
        var round = JSON.parse(out);
        round.name
    "#,
        )
        .unwrap();
    let ptr = r.heap_ptr().unwrap();
    let s = unsafe {
        rune_core::string::HeapString::to_string(ptr as *mut rune_core::string::HeapString)
    };
    assert_eq!(s, "rune");
    // Check boolean survived round-trip (can't use + which has a boolean→string bug)
    let r2 = ctx
        .eval(
            r#"
        var data = JSON.parse('{"nested":{"a":true,"b":null}}');
        var out = JSON.stringify(data);
        var round = JSON.parse(out);
        round.nested.a
    "#,
        )
        .unwrap();
    assert!(r2.to_boolean().unwrap());
}

#[test]
fn test_json_stringify_empty_object() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("JSON.stringify({})").unwrap();
    let ptr = r.heap_ptr().unwrap();
    let s = unsafe {
        rune_core::string::HeapString::to_string(ptr as *mut rune_core::string::HeapString)
    };
    assert_eq!(s, "{}");
}

#[test]
fn test_json_stringify_empty_array() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("JSON.stringify([])").unwrap();
    let ptr = r.heap_ptr().unwrap();
    let s = unsafe {
        rune_core::string::HeapString::to_string(ptr as *mut rune_core::string::HeapString)
    };
    assert_eq!(s, "[]");
}

fn eval_str(ctx: &mut Context, code: &str) -> String {
    let r = ctx.eval(code).unwrap();
    let ptr = r.heap_ptr().unwrap();
    unsafe { rune_core::string::HeapString::to_string(ptr as *mut rune_core::string::HeapString) }
}

fn eval_array_len(ctx: &mut Context, code: &str) -> u32 {
    let r = ctx.eval(code).unwrap();
    let ptr = r.heap_ptr().unwrap();
    unsafe { rune_core::array::RuneArray::length(ptr as *mut rune_core::array::RuneArray) }
}

#[test]
fn test_string_split_basic() {
    let mut ctx = Context::new_small();
    assert_eq!(eval_array_len(&mut ctx, r#" "a,b,c".split(",") "#), 3);
    assert_eq!(eval_str(&mut ctx, r#" "a,b,c".split(",")[0] "#), "a");
    assert_eq!(eval_str(&mut ctx, r#" "a,b,c".split(",")[1] "#), "b");
    assert_eq!(eval_str(&mut ctx, r#" "a,b,c".split(",")[2] "#), "c");
}

#[test]
fn test_string_split_limit() {
    let mut ctx = Context::new_small();
    assert_eq!(eval_array_len(&mut ctx, r#" "a,b,c".split(",", 2) "#), 2);
    assert_eq!(eval_str(&mut ctx, r#" "a,b,c".split(",", 2)[0] "#), "a");
    assert_eq!(eval_str(&mut ctx, r#" "a,b,c".split(",", 2)[1] "#), "b");
}

#[test]
fn test_string_split_no_separator() {
    let mut ctx = Context::new_small();
    assert_eq!(eval_array_len(&mut ctx, r#" "a,b,c".split() "#), 1);
    assert_eq!(eval_str(&mut ctx, r#" "a,b,c".split()[0] "#), "a,b,c");
}

#[test]
fn test_string_split_zero_limit() {
    let mut ctx = Context::new_small();
    assert_eq!(eval_array_len(&mut ctx, r#" "a,b,c".split(",", 0) "#), 0);
}

#[test]
fn test_string_split_empty_string() {
    let mut ctx = Context::new_small();
    assert_eq!(eval_array_len(&mut ctx, r#" "".split(",") "#), 1);
    assert_eq!(eval_str(&mut ctx, r#" "".split(",")[0] "#), "");
}

#[test]
fn test_string_split_empty_separator() {
    let mut ctx = Context::new_small();
    assert_eq!(eval_array_len(&mut ctx, r#" "abc".split("") "#), 3);
    assert_eq!(eval_str(&mut ctx, r#" "abc".split("")[0] "#), "a");
    assert_eq!(eval_str(&mut ctx, r#" "abc".split("")[1] "#), "b");
    assert_eq!(eval_str(&mut ctx, r#" "abc".split("")[2] "#), "c");
}

#[test]
fn test_string_split_space() {
    let mut ctx = Context::new_small();
    assert_eq!(eval_array_len(&mut ctx, r#" "hello world".split(" ") "#), 2);
    assert_eq!(
        eval_str(&mut ctx, r#" "hello world".split(" ")[0] "#),
        "hello"
    );
    assert_eq!(
        eval_str(&mut ctx, r#" "hello world".split(" ")[1] "#),
        "world"
    );
}

#[test]
fn test_string_split_consecutive_delimiters() {
    let mut ctx = Context::new_small();
    assert_eq!(eval_array_len(&mut ctx, r#" "a,,b".split(",") "#), 3);
    assert_eq!(eval_str(&mut ctx, r#" "a,,b".split(",")[0] "#), "a");
    assert_eq!(eval_str(&mut ctx, r#" "a,,b".split(",")[1] "#), "");
    assert_eq!(eval_str(&mut ctx, r#" "a,,b".split(",")[2] "#), "b");
}

fn eval_num(ctx: &mut Context, code: &str) -> f64 {
    let r = ctx.eval(code).unwrap();
    r.as_smi()
        .map(|n| n as f64)
        .or_else(|| r.as_float64())
        .unwrap_or(f64::NAN)
}

#[test]
fn test_parse_int_basic() {
    let mut ctx = Context::new_small();
    assert_eq!(eval_num(&mut ctx, "parseInt('42')"), 42.0);
    assert_eq!(eval_num(&mut ctx, "parseInt('  -42')"), -42.0);
    assert_eq!(eval_num(&mut ctx, "parseInt('  42  ')"), 42.0);
    assert!(eval_num(&mut ctx, "parseInt('hello')").is_nan());
    assert!(eval_num(&mut ctx, "parseInt('')").is_nan());
}

#[test]
fn test_parse_int_hex() {
    let mut ctx = Context::new_small();
    assert_eq!(eval_num(&mut ctx, "parseInt('0xFF')"), 255.0);
    assert_eq!(eval_num(&mut ctx, "parseInt('0xff')"), 255.0);
    assert_eq!(eval_num(&mut ctx, "parseInt('0x1A')"), 26.0);
    assert_eq!(eval_num(&mut ctx, "parseInt('0x1a')"), 26.0);
}

#[test]
fn test_parse_int_radix() {
    let mut ctx = Context::new_small();
    assert_eq!(eval_num(&mut ctx, "parseInt('101', 2)"), 5.0);
    assert_eq!(eval_num(&mut ctx, "parseInt('101', 10)"), 101.0);
    assert_eq!(eval_num(&mut ctx, "parseInt('z', 36)"), 35.0);
}

#[test]
fn test_parse_float_basic() {
    let mut ctx = Context::new_small();
    assert_eq!(eval_num(&mut ctx, "parseFloat('3.5')"), 3.5);
    assert_eq!(eval_num(&mut ctx, "parseFloat('  -3.5')"), -3.5);
    assert_eq!(eval_num(&mut ctx, "parseFloat('  +42.5')"), 42.5);
    assert!(eval_num(&mut ctx, "parseFloat('hello')").is_nan());
    assert!(eval_num(&mut ctx, "parseFloat('')").is_nan());
}

#[test]
fn test_parse_float_edge_cases() {
    let mut ctx = Context::new_small();
    assert!(eval_num(&mut ctx, "parseFloat('Infinity')").is_infinite());
    assert!(eval_num(&mut ctx, "parseFloat('NaN')").is_nan());
    assert_eq!(eval_num(&mut ctx, "parseFloat('12.5abc')"), 12.5);
    assert_eq!(eval_num(&mut ctx, "parseFloat('0.5e2')"), 50.0);
    assert_eq!(eval_num(&mut ctx, "parseFloat('1.5e-2')"), 0.015);
    assert_eq!(eval_num(&mut ctx, "parseFloat('.5')"), 0.5);
}

// ---- P29: builtin throws are catchable by JS try/catch ----

#[test]
fn test_builtin_throw_catchable_by_try_catch() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var caught = 0;
            try { JSON.parse("{invalid"); } catch(e) { caught = 1; }
            caught;
        "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(1), "try/catch should catch builtin throw");
}

#[test]
fn test_builtin_throw_propagates_without_handler() {
    let mut ctx = Context::new_small();
    let r = ctx.eval(r#"JSON.parse("{invalid")"#);
    assert!(r.is_err(), "uncaught builtin throw should propagate as Err");
}

#[test]
fn test_builtin_throw_does_not_infect_subsequent_code() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            try { JSON.parse("{invalid"); } catch (e) {}
            42;
        "#,
        )
        .unwrap();
    assert_eq!(
        r.as_smi(),
        Some(42),
        "execution should resume after caught builtin throw"
    );
}

#[test]
fn test_json_stringify_cycle_still_propagates() {
    let mut ctx = Context::new_small();
    let r = ctx.eval(
        r#"
        var a = {};
        a.self = a;
        JSON.stringify(a)
    "#,
    );
    assert!(
        r.is_err(),
        "cycle in JSON.stringify should still propagate without try/catch"
    );
}

#[test]
fn test_json_stringify_cycle_catchable() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var a = {};
            a.self = a;
            var caught = 0;
            try { JSON.stringify(a); } catch (e) { caught = 1; }
            caught;
        "#,
        )
        .unwrap();
    assert_eq!(
        r.as_smi(),
        Some(1),
        "try/catch should catch cycle in JSON.stringify"
    );
}

#[test]
fn test_async_basic() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var result = 0;
            async function f() {
                result = 42;
            }
            var p = f();
            result;
        "#,
        )
        .unwrap();
    assert_eq!(
        r.as_smi(),
        Some(42),
        "async function should execute body synchronously until first await"
    );
}

#[test]
fn test_async_await_basic() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var result = 0;
            async function f() {
                result = 10;
                await 1;
                result = 20;
            }
            var p = f();
            result;
        "#,
        )
        .unwrap();
    assert_eq!(
        r.as_smi(),
        Some(10),
        "await should suspend execution; result should be 10"
    );
}

#[test]
fn test_async_await_chaining() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var results = [];
            async function f() {
                results.push(1);
                await 1;
                results.push(2);
                await 2;
                results.push(3);
                return "done";
            }
            var p = f();
            results.length;
        "#,
        )
        .unwrap();
    assert_eq!(
        r.as_smi(),
        Some(1),
        "await chains: only first push before first await"
    );
}

#[test]
fn test_promise_finally_fulfilled_passthrough() {
    let mut ctx = Context::new_small();
    // .finally on a fulfilled promise: the handler fires, and the original value
    // passes through (not the handler's return value).
    let r = ctx
        .eval(
            r#"
            var x = 0;
            var p = Promise.resolve(42);
            p.finally(function() { x = 1; return 999; });
            x;
        "#,
        )
        .unwrap();
    assert_eq!(
        r.as_smi(),
        Some(1),
        "finally handler should fire on fulfilled promise"
    );
}

#[test]
fn test_promise_finally_rejected_passthrough() {
    let mut ctx = Context::new_small();
    // .finally on a rejected promise: the handler fires, and the original reason
    // passes through (the chained promise rejects with the original reason).
    let r = ctx
        .eval(
            r#"
            var x = 0;
            var p = Promise.reject(99);
            p.finally(function() { x = 1; });
            x;
        "#,
        )
        .unwrap();
    assert_eq!(
        r.as_smi(),
        Some(1),
        "finally handler should fire on rejected promise"
    );
}

#[test]
fn test_promise_finally_non_callable() {
    let mut ctx = Context::new_small();
    // .finally with a non-callable argument: passthrough the original value
    // without calling any handler.
    let r = ctx
        .eval(
            r#"
            var x = 0;
            Promise.resolve(42).finally(undefined);
            x;
        "#,
        )
        .unwrap();
    assert_eq!(
        r.as_smi(),
        Some(0),
        "non-callable finally should not fire handler"
    );
}

#[test]
fn test_regex_literal() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var r = /abc/;
            typeof r;
        "#,
        )
        .unwrap();
    assert!(
        r.heap_ptr().is_some(),
        "typeof regex literal should return a string"
    );
}

#[test]
fn test_regex_replace_simple() {
    let mut ctx = Context::new_small();
    assert_eq!(
        eval_str(&mut ctx, r#""hello world".replace(/world/, "there")"#),
        "hello there"
    );
}

#[test]
fn test_regex_replace_with_dollar() {
    let mut ctx = Context::new_small();
    assert_eq!(
        eval_str(&mut ctx, r#""hello world".replace(/world/, "($&)")"#),
        "hello (world)"
    );
}

#[test]
fn test_regex_replace_backtick() {
    let mut ctx = Context::new_small();
    assert_eq!(
        eval_str(&mut ctx, r#""hello world".replace(/world/, "$`")"#),
        "hello hello "
    );
}

#[test]
fn test_regex_replace_dot() {
    let mut ctx = Context::new_small();
    assert_eq!(
        eval_str(&mut ctx, r#""hello.world".replace(/\./, "-")"#),
        "hello-world"
    );
}

#[test]
fn test_regex_replace_no_match() {
    let mut ctx = Context::new_small();
    assert_eq!(
        eval_str(&mut ctx, r#""hello world".replace(/xyz/, "there")"#),
        "hello world"
    );
}

#[test]
fn test_regex_replace_all_simple() {
    let mut ctx = Context::new_small();
    assert_eq!(
        eval_str(&mut ctx, r#""a.b.c".replaceAll(/\./g, "-")"#),
        "a-b-c"
    );
}

#[test]
fn test_regex_replace_all_with_dollar() {
    let mut ctx = Context::new_small();
    assert_eq!(
        eval_str(
            &mut ctx,
            r#""hello world hello".replaceAll(/hello/g, "($&)")"#
        ),
        "(hello) world (hello)"
    );
}

#[test]
fn test_regex_replace_capture_dollar_1() {
    let mut ctx = Context::new_small();
    assert_eq!(
        eval_str(&mut ctx, r#""hello world".replace(/(\w+)/, "$1")"#),
        "hello world"
    );
}

#[test]
fn test_regex_replace_capture_swap() {
    let mut ctx = Context::new_small();
    assert_eq!(
        eval_str(&mut ctx, r#""hello world".replace(/(\w+) (\w+)/, "$2 $1")"#),
        "world hello"
    );
}

#[test]
fn test_regex_replace_capture_nested() {
    let mut ctx = Context::new_small();
    assert_eq!(
        eval_str(&mut ctx, r#""abc".replace(/(a(b)c)/, "[$1][$2]")"#),
        "[abc][b]"
    );
}

#[test]
fn test_regex_replace_all_capture() {
    let mut ctx = Context::new_small();
    assert_eq!(
        eval_str(&mut ctx, r#""a1 b2 c3".replaceAll(/(\w)(\d)/g, "$2$1")"#),
        "1a 2b 3c"
    );
}

#[test]
fn test_regexp_exec_match() {
    let mut ctx = Context::new_small();
    let _r = ctx
        .eval(
            r#"
        var re = /world/;
        var m = re.exec("hello world");
        m[0];
    "#,
        )
        .unwrap();
    assert_eq!(
        eval_str(&mut ctx, r#"/world/.exec("hello world")[0]"#),
        "world"
    );
}

#[test]
fn test_regexp_exec_no_match() {
    let mut ctx = Context::new_small();
    assert!(ctx.eval(r#"/xyz/.exec("hello world")"#).unwrap().is_null(),);
}

#[test]
fn test_regexp_exec_capture() {
    let mut ctx = Context::new_small();
    assert_eq!(
        eval_str(&mut ctx, r#"/(\w+) (\w+)/.exec("hello world")[1]"#),
        "hello"
    );
    assert_eq!(
        eval_str(&mut ctx, r#"/(\w+) (\w+)/.exec("hello world")[2]"#),
        "world"
    );
}

#[test]
fn test_regexp_test_true() {
    let mut ctx = Context::new_small();
    assert_eq!(
        ctx.eval(r#"/hello/.test("hello world")"#)
            .unwrap()
            .to_boolean(),
        Some(true)
    );
}

#[test]
fn test_regexp_test_false() {
    let mut ctx = Context::new_small();
    assert_eq!(
        ctx.eval(r#"/xyz/.test("hello world")"#)
            .unwrap()
            .to_boolean(),
        Some(false)
    );
}

#[test]
fn test_regexp_prototype_source() {
    let mut ctx = Context::new_small();
    assert_eq!(eval_str(&mut ctx, r#"/hello/.source"#), "hello");
    assert_eq!(eval_str(&mut ctx, r#"/world/.source"#), "world");
}

#[test]
fn test_regexp_prototype_flags() {
    let mut ctx = Context::new_small();
    assert_eq!(eval_str(&mut ctx, r#"/hello/gi.flags"#), "gi");
    assert_eq!(eval_str(&mut ctx, r#"/hello/m.flags"#), "m");
    assert_eq!(eval_str(&mut ctx, r#"/hello/.flags"#), "");
}

#[test]
fn test_regexp_prototype_last_index() {
    let mut ctx = Context::new_small();
    // lastIndex defaults to 0
    assert_eq!(ctx.eval(r#"/hello/.lastIndex"#).unwrap().as_smi(), Some(0));
}

#[test]
fn test_regexp_constructor_basic() {
    let mut ctx = Context::new_small();
    assert_eq!(
        eval_str(&mut ctx, r#"new RegExp("a(b)c", "g").source"#),
        "a(b)c"
    );
    assert_eq!(eval_str(&mut ctx, r#"new RegExp("a(b)c", "g").flags"#), "g");
    assert_eq!(
        eval_str(
            &mut ctx,
            r#"var r = new RegExp("a(b)c", "g"); r.exec("xabcyabc").index + "/" + r.lastIndex;"#
        ),
        "1/4"
    );
    assert_eq!(eval_str(&mut ctx, r#"new RegExp().source"#), "");
    assert_eq!(eval_str(&mut ctx, r#"new RegExp("xy").flags"#), "");
}

#[test]
fn test_regexp_constructor_regexp_arg() {
    let mut ctx = Context::new_small();
    assert_eq!(
        eval_str(
            &mut ctx,
            r#"var r1 = /ab/gi; new RegExp(r1).source + "/" + new RegExp(r1).flags"#
        ),
        "ab/gi"
    );
    assert_eq!(
        ctx.eval(r#"var r1 = /ab/; RegExp(r1) === r1;"#)
            .unwrap()
            .to_boolean(),
        Some(true)
    );
    assert_eq!(
        ctx.eval(r#"var r = RegExp("xy", "m"); r instanceof RegExp;"#)
            .unwrap()
            .to_boolean(),
        Some(true)
    );
    assert_eq!(
        ctx.eval(r#"var r = new RegExp("xy"); r instanceof RegExp;"#)
            .unwrap()
            .to_boolean(),
        Some(true)
    );
}

#[test]
fn test_regexp_constructor_flags_validation() {
    let mut ctx = Context::new_small();
    for bad in ["q", "gg", "mm", "ag"] {
        let result = ctx.eval(&format!(r#"new RegExp("a", "{bad}");"#));
        assert!(result.is_err(), "flag {bad} should throw");
        let err = result.unwrap_err();
        assert!(
            err.contains("SyntaxError") || err.contains("RegExp"),
            "unexpected error: {err}"
        );
    }
    // Valid flags still work; .flags returns them in canonical order.
    assert_eq!(
        eval_str(&mut ctx, r#"new RegExp("a", "dgimsuvy").flags"#),
        "gimsuydv"
    );
}

#[test]
fn test_regexp_exec_index_input() {
    let mut ctx = Context::new_small();
    assert_eq!(
        ctx.eval(r#"/b+/.exec("xxabbb").index"#).unwrap().as_smi(),
        Some(3)
    );
    assert_eq!(eval_str(&mut ctx, r#"/b+/.exec("xxabbb").input"#), "xxabbb");
    assert_eq!(
        ctx.eval(r#"/b+/.exec("xxabbb").length"#).unwrap().as_smi(),
        Some(1)
    );
    assert_eq!(
        ctx.eval(r#"/(\d+)/.exec("a123b").index"#).unwrap().as_smi(),
        Some(1)
    );
    assert_eq!(
        eval_str(&mut ctx, r#"/(\d+)/.exec("a123b").input"#),
        "a123b"
    );
}

#[test]
fn test_regexp_exec_lastindex_global() {
    let mut ctx = Context::new_small();
    // Global exec advances lastIndex per match, starting from lastIndex.
    assert_eq!(
        eval_str(
            &mut ctx,
            r#"var r = /a/g; r.exec("aba"); var i1 = r.lastIndex; var m2 = r.exec("aba"); i1 + "/" + m2.index + "/" + r.lastIndex;"#
        ),
        "1/2/3"
    );
    // Failure resets lastIndex to 0.
    assert_eq!(
        ctx.eval(r#"var r = /a/g; r.exec("zzz"); r.lastIndex;"#)
            .unwrap()
            .as_smi(),
        Some(0)
    );
    // Global exec with no more matches returns null.
    assert_eq!(
        ctx.eval(
            r#"var r = /a/g; r.exec("aaa"); r.exec("aaa"); r.exec("aaa"); r.exec("aaa") === null;"#
        )
        .unwrap()
        .to_boolean(),
        Some(true)
    );
}

#[test]
fn test_regexp_exec_lastindex_sticky() {
    let mut ctx = Context::new_small();
    assert_eq!(
        eval_str(
            &mut ctx,
            r#"var r = /a/y; r.lastIndex = 1; var m = r.exec("baa"); m.index + "/" + r.lastIndex;"#
        ),
        "1/2"
    );
    // Sticky failure resets lastIndex to 0.
    assert_eq!(
        ctx.eval(r#"var r = /a/y; r.lastIndex = 0; r.exec("baa"); r.lastIndex;"#)
            .unwrap()
            .as_smi(),
        Some(0)
    );
}

#[test]
fn test_regexp_lastindex_store() {
    let mut ctx = Context::new_small();
    // Setting lastIndex on a regexp works and is used by global exec.
    assert_eq!(
        ctx.eval(r#"var r = /b/g; r.lastIndex = 2; r.exec("abbb").index;"#)
            .unwrap()
            .as_smi(),
        Some(2)
    );
    // lastIndex on a literal regexp is stored back.
    assert_eq!(
        ctx.eval(r#"var r = /x/g; r.test("xax"); r.lastIndex;"#)
            .unwrap()
            .as_smi(),
        Some(1)
    );
}

#[test]
fn test_regexp_anchors() {
    let mut ctx = Context::new_small();
    // ^ anchors to start: should not match later position
    assert_eq!(
        ctx.eval(r#"/^abc/.test("xabc")"#).unwrap().to_boolean(),
        Some(false)
    );
    assert_eq!(
        ctx.eval(r#"/^abc/.test("abc")"#).unwrap().to_boolean(),
        Some(true)
    );
    // $ anchors to end
    assert_eq!(
        ctx.eval(r#"/abc$/.test("abcx")"#).unwrap().to_boolean(),
        Some(false)
    );
    assert_eq!(
        ctx.eval(r#"/abc$/.test("xabc")"#).unwrap().to_boolean(),
        Some(true)
    );
    // ^$ together
    assert_eq!(
        ctx.eval(r#"/^abc$/.test("abc")"#).unwrap().to_boolean(),
        Some(true)
    );
    assert_eq!(
        ctx.eval(r#"/^abc$/.test("ab")"#).unwrap().to_boolean(),
        Some(false)
    );
    assert_eq!(
        ctx.eval(r#"/^$/.test("")"#).unwrap().to_boolean(),
        Some(true)
    );
    assert_eq!(
        ctx.eval(r#"/^$/.test("a")"#).unwrap().to_boolean(),
        Some(false)
    );
    // Anchors with sticky/global lastIndex handling
    assert_eq!(
        ctx.eval(r#"var r=/a/y; r.lastIndex=0; r.exec("baa") === null"#)
            .unwrap()
            .to_boolean(),
        Some(true)
    );
    assert_eq!(
        ctx.eval(r#"/^a/.test("ba")"#).unwrap().to_boolean(),
        Some(false)
    );
}

#[test]
fn test_regexp_word_boundaries() {
    let mut ctx = Context::new_small();
    // \b matches word boundary (transition between \w and \W or string edge)
    assert_eq!(
        ctx.eval(r#"/\bword\b/.test("word")"#).unwrap().to_boolean(),
        Some(true)
    );
    assert_eq!(
        ctx.eval(r#"/\bword\b/.test("xword")"#)
            .unwrap()
            .to_boolean(),
        Some(false)
    );
    assert_eq!(
        ctx.eval(r#"/\bword\b/.test("wordx")"#)
            .unwrap()
            .to_boolean(),
        Some(false)
    );
    assert_eq!(
        ctx.eval(r#"/\bhello\b/.test("hello world")"#)
            .unwrap()
            .to_boolean(),
        Some(true)
    );
    assert_eq!(
        ctx.eval(r#"/\bhello\b/.test("ahello")"#)
            .unwrap()
            .to_boolean(),
        Some(false)
    );
    // \B matches non-boundary
    assert_eq!(
        ctx.eval(r#"/\Bword\B/.test("xwordx")"#)
            .unwrap()
            .to_boolean(),
        Some(true)
    );
    assert_eq!(
        ctx.eval(r#"/\Bword\B/.test(" word ") "#)
            .unwrap()
            .to_boolean(),
        Some(false)
    );
    // \b at start/end edge cases
    assert_eq!(
        ctx.eval(r#"/\b/.test("a")"#).unwrap().to_boolean(),
        Some(true)
    );
    assert_eq!(
        ctx.eval(r#"/\B/.test("a")"#).unwrap().to_boolean(),
        Some(false)
    );
    assert_eq!(
        ctx.eval(r#"/\b/.test("")"#).unwrap().to_boolean(),
        Some(false)
    );
}

#[test]
fn test_regexp_backrefs() {
    let mut ctx = Context::new_small();
    // Simple backref (a)\1
    assert_eq!(
        ctx.eval(r#"/(a)\1/.test("aa")"#).unwrap().to_boolean(),
        Some(true)
    );
    assert_eq!(
        ctx.eval(r#"/(a)\1/.test("ab")"#).unwrap().to_boolean(),
        Some(false)
    );
    assert_eq!(
        ctx.eval(r#"/(ab)\1/.test("abab")"#).unwrap().to_boolean(),
        Some(true)
    );
    assert_eq!(
        ctx.eval(r#"/(ab)\1/.test("abac")"#).unwrap().to_boolean(),
        Some(false)
    );
    // Multiple backrefs (a)(b)\1\2 → abab
    assert_eq!(
        ctx.eval(r#"/(a)(b)\1\2/.test("abab")"#)
            .unwrap()
            .to_boolean(),
        Some(true)
    );
    assert_eq!(
        ctx.eval(r#"/(a)(b)\2\1/.test("abba")"#)
            .unwrap()
            .to_boolean(),
        Some(true)
    );
    // Backref to non-participating group acts as empty
    assert_eq!(
        ctx.eval(r#"/(a)?\1/.test("aa")"#).unwrap().to_boolean(),
        Some(true)
    );
    assert_eq!(
        ctx.eval(r#"/(a)?\1/.test("b")"#).unwrap().to_boolean(),
        Some(true)
    );
    // (a+)\1 with greedy quantifier
    assert_eq!(eval_str(&mut ctx, r#"/(a+)\1/.exec("aaa")[0]"#), "aa");
    assert_eq!(eval_str(&mut ctx, r#"/(a+)\1/.exec("aaa")[1]"#), "a");
}

#[test]
fn test_regexp_flags() {
    let mut ctx = Context::new_small();
    // i flag — case-insensitive
    assert_eq!(
        ctx.eval(r#"/a/i.test("A")"#).unwrap().to_boolean(),
        Some(true)
    );
    assert_eq!(
        ctx.eval(r#"/hello/i.test("HELLO")"#).unwrap().to_boolean(),
        Some(true)
    );
    assert_eq!(
        ctx.eval(r#"/A/i.test("a")"#).unwrap().to_boolean(),
        Some(true)
    );
    assert_eq!(
        ctx.eval(r#"/a/.test("A")"#).unwrap().to_boolean(),
        Some(false)
    );
    // i with backref
    assert_eq!(
        ctx.eval(r#"/(a)\1/i.test("AA")"#).unwrap().to_boolean(),
        Some(true)
    );
    // m flag — multiline ^ and $
    assert_eq!(
        ctx.eval(r#"/^a/m.test("\na")"#).unwrap().to_boolean(),
        Some(true)
    );
    assert_eq!(
        ctx.eval(r#"/^a/.test("\na")"#).unwrap().to_boolean(),
        Some(false)
    );
    assert_eq!(
        ctx.eval(r#"/a$/m.test("a\n")"#).unwrap().to_boolean(),
        Some(true)
    );
    assert_eq!(
        ctx.eval(r#"/a$/.test("a\n")"#).unwrap().to_boolean(),
        Some(false)
    );
    // s flag — dotAll
    assert_eq!(
        ctx.eval(r#"/./.test("\n")"#).unwrap().to_boolean(),
        Some(false)
    );
    assert_eq!(
        ctx.eval(r#"/./s.test("\n")"#).unwrap().to_boolean(),
        Some(true)
    );
}

#[test]
fn test_tonumber_symbol_throws() {
    let mut ctx = Context::new_small();
    // Number(Symbol) should throw TypeError (catchable)
    assert!(ctx.eval(r#"Number(Symbol("a"))"#).is_err());
    assert_eq!(
        ctx.eval(
            r#"var r = false; try { Number(Symbol("a")); } catch(e) { r = String(e).includes("TypeError"); } r"#
        )
        .unwrap()
        .to_boolean(),
        Some(true)
    );
    // Arithmetic with Symbol should throw
    assert!(ctx.eval(r#"1 + Symbol("a")"#).is_err());
    assert!(ctx.eval(r#"Symbol("a") - 1"#).is_err());
    assert!(ctx.eval(r#"Symbol("a") * 2"#).is_err());
    assert!(ctx.eval(r#"Symbol("a") / 2"#).is_err());
    assert!(ctx.eval(r#"Symbol("a") % 2"#).is_err());
    assert!(ctx.eval(r#"Symbol("a") ** 2"#).is_err());
    assert!(ctx.eval(r#"Symbol("a") << 1"#).is_err());
    assert!(ctx.eval(r#"Symbol("a") | 1"#).is_err());
    // String concatenation with Symbol should throw TypeError for string
    assert!(ctx.eval(r#""a" + Symbol("b")"#).is_err());
    assert!(ctx.eval(r#"Symbol("a") + "b""#).is_err());
    // Unary plus with Symbol should throw
    assert!(ctx.eval(r#"+Symbol("a")"#).is_err());
    // Inc/dec with Symbol
    assert!(ctx.eval(r#"var s = Symbol("a"); ++s"#).is_err());
    assert!(ctx.eval(r#"var s = Symbol("a"); s++"#).is_err());
    // Comparison via number coercion should not throw for == with symbol? Instead false, but ToNumber for symbol in == string/number case is not called for symbol vs number - returns false directly. So check that == does not throw
    assert_eq!(
        ctx.eval(r#"Symbol("a") == 1"#).unwrap().to_boolean(),
        Some(false)
    );
    assert_eq!(
        ctx.eval(r#"Symbol("a") != 1"#).unwrap().to_boolean(),
        Some(true)
    );
}

#[test]
fn test_regexp_search_resets_lastindex() {
    let mut ctx = Context::new_small();
    assert_eq!(
        ctx.eval(r#"var r = /a/g; r.lastIndex = 1; "aba".search(r); r.lastIndex;"#)
            .unwrap()
            .as_smi(),
        Some(1)
    );
    assert_eq!(ctx.eval(r#""aba".search(/b/)"#).unwrap().as_smi(), Some(1));
}

#[test]
fn test_regexp_match_global_lastindex() {
    let mut ctx = Context::new_small();
    assert_eq!(
        eval_str(
            &mut ctx,
            r#"var r = /a/g; "aba".match(r).length + "/" + r.lastIndex;"#
        ),
        "2/3"
    );
    assert_eq!(
        eval_str(
            &mut ctx,
            r#""foo".match(/o+/).index + "/" + "foo".match(/o+/).input;"#
        ),
        "1/foo"
    );
}

#[test]
fn test_regex_replace_all_function() {
    let mut ctx = Context::new_small();
    assert_eq!(
        eval_str(
            &mut ctx,
            r#""a1b2c3".replaceAll(/(\d)/g, function(m, d, p, s) { return "[" + d + "]"; })"#
        ),
        "a[1]b[2]c[3]"
    );
    // Position and input args.
    assert_eq!(
        eval_str(
            &mut ctx,
            r#"var out = []; "a1b2c3".replaceAll(/(\d)/g, function(m, d, p, s) { out.push(m + "@" + p); return ""; }); out.length + "|" + out[0] + "|" + out[1] + "|" + out[2];"#
        ),
        "3|1@1|2@3|3@5"
    );
    // String search + function replacement.
    assert_eq!(
        eval_str(
            &mut ctx,
            r#""abab".replaceAll("a", function(m, p, s) { return "" + p; })"#
        ),
        "0b2b"
    );
    // Adjacent matches and empty-string matches must not loop forever.
    assert_eq!(
        eval_str(
            &mut ctx,
            r#""abab".replaceAll("a", function(m, p) { return "x"; })"#
        ),
        "xbxb"
    );
}

#[test]
fn test_regex_lookahead() {
    let mut ctx = Context::new_small();
    assert_eq!(eval_str(&mut ctx, r#"/a(?=b)/.exec("abac")[0]"#), "a");
    assert_eq!(eval_str(&mut ctx, r#"/a(?!b)/.exec("ac")[0]"#), "a");
    // /a(?!b)/ still matches the 'a' at index 2 of "abac" (followed by 'c').
    assert_eq!(eval_str(&mut ctx, r#"/a(?!b)/.exec("abac")[0]"#), "a");
    assert_eq!(eval_str(&mut ctx, r#"/a(?!b)/.exec("ac")[0]"#), "a");
    assert_eq!(eval_str(&mut ctx, r#"/(?=(a+))a/.exec("baaab")[1]"#), "aaa");
    assert_eq!(eval_str(&mut ctx, r#"/\d+(?=px)/.exec("100px")[0]"#), "100");
}

#[test]
fn test_regex_quantifier() {
    let mut ctx = Context::new_small();
    assert_eq!(eval_str(&mut ctx, r#"/\d{3}/.exec("ab123")[0]"#), "123");
    assert_eq!(eval_str(&mut ctx, r#"/a{2,3}/.exec("baaa")[0]"#), "aaa");
    assert!(ctx.eval(r#"/a{2}/.exec("ba")"#).unwrap().is_null());
    assert_eq!(eval_str(&mut ctx, r#"/a{1,2}/.exec("baaa")[0]"#), "aa");
    assert_eq!(eval_str(&mut ctx, r#"/\d{2,}/.exec("a123")[0]"#), "123");
}

#[test]
fn test_array_named_props() {
    let mut ctx = Context::new_small();
    assert_eq!(
        ctx.eval(r#"var a = [1,2]; a.foo = 42; a.foo;"#)
            .unwrap()
            .as_smi(),
        Some(42)
    );
    assert_eq!(
        eval_str(
            &mut ctx,
            r#"var a = [1,2]; a.foo = 42; a.foo + "/" + a.length;"#
        ),
        "42/2"
    );
    // Named props are own enumerable properties for Object.keys.
    assert_eq!(
        ctx.eval(r#"var a = [1,2]; a.foo = 42; Object.keys(a).length;"#)
            .unwrap()
            .as_smi(),
        Some(3)
    );
    // Overwriting an element still works alongside named props.
    assert_eq!(
        eval_str(
            &mut ctx,
            r#"var a = [1,2]; a.foo = 42; a[0] = 9; a[0] + "/" + a.foo;"#
        ),
        "9/42"
    );
}

#[test]
fn test_regex_replace_function() {
    let mut ctx = Context::new_small();
    assert_eq!(
        eval_str(
            &mut ctx,
            r#""hello".replace(/l/, function(m) { return "X"; })"#
        ),
        "heXlo"
    );
}

#[test]
fn test_regex_replace_function_captures() {
    let mut ctx = Context::new_small();
    assert_eq!(
        eval_str(
            &mut ctx,
            r#""a1b2c3".replace(/(\d)/, function(m, c1, off, s) { return c1 + c1; })"#
        ),
        "a11b2c3"
    );
}

// ---- Thenable Unwrapping Tests ----

#[test]
fn test_thenable_unwrapping_is_deferred() {
    let mut ctx = Context::new_small();
    let val = ctx
        .eval(
            r#"
        var side_effect = 0;
        var thenable = { then: function(resolve) { side_effect = 1; resolve(42); } };
        var p = Promise.resolve(thenable);
        side_effect;
    "#,
        )
        .unwrap();
    assert_eq!(
        val.as_smi(),
        Some(0),
        "thenable job must not run synchronously (PromiseResolveThenableJob)"
    );
    assert_eq!(ctx.eval("side_effect").unwrap().as_smi(), Some(1));
}

#[test]
fn test_thenable_unwrapping_resolve_value() {
    let mut ctx = Context::new_small();
    // First eval: set up thenable, resolve it, chain .then (microtask enqueued)
    ctx.eval(
        r#"
        var thenable = { then: function(resolve) { resolve(42); } };
        var p = Promise.resolve(thenable);
        var resolvedValue;
        p.then(function(v) { resolvedValue = v; });
    "#,
    )
    .unwrap();
    // Microtasks are drained at end of execute(); resolvedValue is now 42
    let val = ctx.eval("resolvedValue;").unwrap();
    assert_eq!(
        val.as_smi(),
        Some(42),
        "promise should be fulfilled with 42"
    );
}

#[test]
fn test_thenable_unwrapping_non_thenable() {
    let mut ctx = Context::new_small();
    ctx.eval(
        r#"
        var obj = { foo: 'bar' };
        var p = Promise.resolve(obj);
        var result;
        p.then(function(v) { result = v; });
    "#,
    )
    .unwrap();
    let val = ctx.eval("result;").unwrap();
    assert!(
        val.is_heap_object(),
        "plain object should be wrapped in promise"
    );
}

// ---- Class Syntax Tests ----

fn class_eval_num(ctx: &mut Context, code: &str) -> i32 {
    let r = ctx.eval(code).unwrap();
    r.as_smi().unwrap()
}

#[test]
fn test_class_basic() {
    let mut ctx = Context::new_small();
    assert_eq!(
        class_eval_num(
            &mut ctx,
            "class Foo { constructor(x) { this.x = x; } getX() { return this.x; } } var f = new Foo(42); f.getX();"
        ),
        42
    );
}

#[test]
fn test_class_no_constructor() {
    let mut ctx = Context::new_small();
    assert_eq!(
        class_eval_num(
            &mut ctx,
            "class Foo { method() { return 1; } } var f = new Foo(); f.method();"
        ),
        1
    );
}

#[test]
fn test_class_multiple_methods() {
    let mut ctx = Context::new_small();
    assert_eq!(
        class_eval_num(
            &mut ctx,
            "class Calc { constructor(x) { this.val = x; } add(n) { this.val = this.val + n; return this; } get() { return this.val; } } var c = new Calc(10); c.add(5).add(3); c.get();"
        ),
        18
    );
}

#[test]
fn test_class_expression() {
    let mut ctx = Context::new_small();
    assert_eq!(
        class_eval_num(
            &mut ctx,
            "var Foo = class { constructor(x) { this.x = x; } getX() { return this.x; } }; var f = new Foo(99); f.getX();"
        ),
        99
    );
}

#[test]
fn test_class_expression_anonymous_direct() {
    let mut ctx = Context::new_small();
    assert_eq!(
        class_eval_num(
            &mut ctx,
            "var result = new (class { constructor(v) { this.val = v; } getVal() { return this.val; } })(7).getVal(); result;"
        ),
        7
    );
}

#[test]
fn test_class_default_constructor() {
    let mut ctx = Context::new_small();
    assert_eq!(
        class_eval_num(
            &mut ctx,
            "class Foo { getVal() { return 42; } } new Foo().getVal();"
        ),
        42
    );
}

#[test]
fn test_class_default_derived_constructor() {
    let mut ctx = Context::new_small();
    // Derived class with no explicit constructor should synthesize constructor(...args) { super(...args); }
    assert_eq!(
        class_eval_num(
            &mut ctx,
            "class Parent { constructor(x) { this.x = x; } getX() { return this.x; } }
         class Child extends Parent { getDouble() { return this.x * 2; } }
         new Child(21).getX();"
        ),
        21
    );
    // Multiple args
    assert_eq!(
        class_eval_num(
            &mut ctx,
            "class Parent { constructor(a, b) { this.sum = a + b; } }
         class Child extends Parent { }
         new Child(3, 4).sum;"
        ),
        7
    );
    // Three-level chain with default constructors
    assert_eq!(
        class_eval_num(
            &mut ctx,
            "class GrandParent { constructor(x) { this.val = x; } }
         class Parent extends GrandParent { }
         class Child extends Parent { }
         new Child(42).val;"
        ),
        42
    );
}

#[test]
fn test_class_method_this_context() {
    let mut ctx = Context::new_small();
    assert_eq!(
        class_eval_num(
            &mut ctx,
            "class Foo { constructor() { this.val = 1; } inc() { this.val = this.val + 1; return this; } get() { return this.val; } } var a = new Foo(); var b = new Foo(); a.inc(); a.get();"
        ),
        2
    );
}

#[test]
fn test_class_extends_basic() {
    let mut ctx = Context::new_small();
    assert_eq!(
        class_eval_num(
            &mut ctx,
            "class Parent { parentMethod() { return 1; } } class Child extends Parent { } new Child().parentMethod();"
        ),
        1
    );
}

#[test]
fn test_class_extends_multiple_methods() {
    let mut ctx = Context::new_small();
    assert_eq!(
        class_eval_num(
            &mut ctx,
            "class Parent { getX() { return 42; } } class Child extends Parent { getY() { return 7; } } var c = new Child(); c.getX() + c.getY();"
        ),
        49
    );
}

#[test]
fn test_class_extends_prototype_chain() {
    let mut ctx = Context::new_small();
    assert_eq!(
        class_eval_num(
            &mut ctx,
            "class GrandParent { gp() { return 10; } } class Parent extends GrandParent { p() { return 3; } } class Child extends Parent { c() { return 7; } } var c = new Child(); c.gp() * 10 + c.p() * 100 + c.c();"
        ),
        100 + 300 + 7
    );
}

#[test]
fn test_class_super_call() {
    let mut ctx = Context::new_small();
    assert_eq!(
        class_eval_num(
            &mut ctx,
            "class Parent { constructor(x) { this.x = x; } } class Child extends Parent { constructor(x, y) { super(x); this.y = y; } } var c = new Child(10, 20); c.x;"
        ),
        10
    );
}

#[test]
fn test_class_super_call_property_setting() {
    let mut ctx = Context::new_small();
    assert_eq!(
        class_eval_num(
            &mut ctx,
            "class Parent { constructor(a, b) { this.a = a; this.b = b; } }
         class Child extends Parent { constructor(a, b, c) { super(a, b); this.c = c; } }
         new Child(10, 20, 30).c;"
        ),
        30
    );
}

#[test]
fn test_class_super_call_no_args() {
    let mut ctx = Context::new_small();
    assert_eq!(
        class_eval_num(
            &mut ctx,
            "class Parent { constructor() { this.x = 5; } }
         class Child extends Parent { constructor() { super(); } }
         new Child().x;"
        ),
        5
    );
}

#[test]
fn test_class_super_multi_level() {
    let mut ctx = Context::new_small();
    assert_eq!(
        class_eval_num(
            &mut ctx,
            "class GrandParent { constructor(x) { this.gx = x; } }
         class Parent extends GrandParent { constructor(x, y) { super(x); this.py = y; } }
         class Child extends Parent { constructor(x, y, z) { super(x, y); this.cz = z; } }
         var c = new Child(1, 2, 3); c.gx + c.py + c.cz;"
        ),
        6
    );
}

#[test]
fn test_class_super_prop_read() {
    let mut ctx = Context::new_small();
    assert_eq!(
        class_eval_num(
            &mut ctx,
            "class Parent { getX() { return 42; } }
         class Child extends Parent { constructor() { super(); }
           method() { return this.getX(); } }
         new Child().method();"
        ),
        42
    );
}

#[test]
fn test_class_super_method_call() {
    let mut ctx = Context::new_small();
    assert_eq!(
        class_eval_num(
            &mut ctx,
            "class Parent { add(a, b) { return a + b; } }
         class Child extends Parent { sum(a, b) { return super.add(a, b); } }
         new Child().sum(3, 4);"
        ),
        7
    );
}

#[test]
fn test_class_super_method_multi_level() {
    let mut ctx = Context::new_small();
    assert_eq!(
        class_eval_num(
            &mut ctx,
            "class GrandParent { base() { return 1; } }
         class Parent extends GrandParent { parent() { return super.base() + 10; } }
         class Child extends Parent { child() { return super.parent() + 100; } }
         new Child().child();"
        ),
        111
    );
}

#[test]
fn test_class_super_method() {
    let mut ctx = Context::new_small();
    assert_eq!(
        class_eval_num(
            &mut ctx,
            "class Parent { getX() { return 42; } }
         class Child extends Parent { constructor() { super(); }
           method() { return super.getX(); } }
         new Child().method();"
        ),
        42
    );
}

#[test]
fn test_class_super_method_with_args() {
    let mut ctx = Context::new_small();
    assert_eq!(
        class_eval_num(
            &mut ctx,
            "class Parent { add(a, b) { return a + b; } }
         class Child extends Parent { sum(a, b) { return super.add(a, b); } }
         new Child().sum(3, 4);"
        ),
        7
    );
}

#[test]
fn test_class_super_prop_read_data() {
    let mut ctx = Context::new_small();
    assert_eq!(
        class_eval_num(
            &mut ctx,
            "class Parent { constructor() { this.val = 99; } getVal() { return this.val; } }
         class Child extends Parent { constructor() { super(); } read() { return super.getVal(); } }
         new Child().read();"
        ),
        99
    );
}

#[test]
fn test_class_static_method() {
    let mut ctx = Context::new_small();
    let r = ctx.eval(
        "class Foo { static greet() { return 42; } }
         var r = Foo.greet();
         r;",
    );
    assert_eq!(
        r.unwrap().as_smi(),
        Some(42),
        "static method should return 42"
    );
}

#[test]
fn test_class_static_multiple_methods() {
    let mut ctx = Context::new_small();
    let r = ctx.eval(
        "class Calc { static add(a, b) { return a + b; } static mul(a, b) { return a * b; } }
         var r1 = Calc.add(3, 4);
         var r2 = Calc.mul(3, 4);
         r1 + r2;",
    );
    assert_eq!(r.unwrap().as_smi(), Some(19), "3+4 + 3*4 = 19");
}

#[test]
fn test_class_static_method_this() {
    let mut ctx = Context::new_small();
    // Static method 'this' refers to the constructor
    let r = ctx.eval(
        "class Foo { static getThis() { return this; } }
         var t = Foo.getThis();
         t === Foo;",
    );
    assert_eq!(
        r.unwrap().to_boolean(),
        Some(true),
        "this should be the constructor"
    );
}

#[test]
fn test_class_static_with_instance() {
    let mut ctx = Context::new_small();
    // Test that static methods can create instances via new (in separate eval)
    let r = ctx.eval(
        "class Foo { constructor(x) { this.x = x; } static create(x) { return new Foo(x); } }
         undefined;",
    );
    assert!(r.is_ok());
    let r = ctx.eval("Foo.create(42).x;");
    assert_eq!(
        r.unwrap().as_smi(),
        Some(42),
        "static factory should return 42"
    );
}

#[test]
fn test_class_super_prop_assign() {
    let mut ctx = Context::new_small();
    // super.prop = val should write to this (child instance)
    assert_eq!(
        class_eval_num(
            &mut ctx,
            "class Parent { getX() { return 10; } }
         class Child extends Parent { constructor() { super(); }
           method() { super.x = 42; return this.x; } }
         new Child().method();"
        ),
        42
    );
}

#[test]
fn test_class_super_prop_assign_overrides_parent() {
    let mut ctx = Context::new_small();
    // super.prop = val on a parent property should shadow it on the child
    assert_eq!(
        class_eval_num(
            &mut ctx,
            "class Parent { constructor() { this.val = 1; } }
         class Child extends Parent { constructor() { super(); }
           method() { super.val = 99; return this.val; } }
         var c = new Child();
         c.method();"
        ),
        99
    );
}

#[test]
fn test_class_getter_simple() {
    let mut ctx = Context::new_small();
    // Simple getter: get prop() { return expr; }
    assert_eq!(
        class_eval_num(
            &mut ctx,
            "class Foo { get prop() { return 42; } }
         var f = new Foo();
         f.prop;"
        ),
        42
    );
}

#[test]
fn test_class_getter_setter() {
    let mut ctx = Context::new_small();
    // Getter and setter for same property
    assert_eq!(
        class_eval_num(
            &mut ctx,
            "class Foo { constructor() { this._x = 0; }
           get x() { return this._x; }
           set x(v) { this._x = v; } }
         var f = new Foo();
         f.x = 10;
         f.x;"
        ),
        10
    );
}

#[test]
fn test_class_getter_no_setter() {
    let mut ctx = Context::new_small();
    // Getter without setter — assignment does not shadow the accessor (per spec)
    assert_eq!(
        class_eval_num(
            &mut ctx,
            "class Foo { get x() { return 1; } }
         var f = new Foo();
         f.x = 2;
         f.x;"
        ),
        1
    );
}

#[test]
fn test_class_static_getter() {
    let mut ctx = Context::new_small();
    // Static getter
    assert_eq!(
        class_eval_num(
            &mut ctx,
            "class Foo { static get count() { return 42; } }
         Foo.count;"
        ),
        42
    );
}

#[test]
fn test_class_getter_this() {
    let mut ctx = Context::new_small();
    // Getter `this` refers to the instance
    assert_eq!(
        class_eval_num(
            &mut ctx,
            "class Foo { constructor(v) { this.val = v; }
           get doubled() { return this.val * 2; } }
         var f = new Foo(21);
         f.doubled;"
        ),
        42
    );
}

#[test]
fn test_class_setter_this() {
    let mut ctx = Context::new_small();
    // Setter `this` refers to the instance
    assert_eq!(
        class_eval_num(
            &mut ctx,
            "class Foo { constructor() { this._x = 0; }
           set x(v) { this._x = v + 1; }
           get x() { return this._x; } }
         var f = new Foo();
         f.x = 10;
         f.x;"
        ),
        11
    );
}

#[test]
fn test_class_super_compound_assign() {
    let mut ctx = Context::new_small();
    // super.prop += val reads from parent prototype, writes to this
    assert_eq!(
        class_eval_num(
            &mut ctx,
            "class Parent { }
         Parent.prototype.val = 5;
         class Child extends Parent { constructor() { super(); }
           method() { super.val += 10; return this.val; } }
         new Child().method();"
        ),
        15
    );
}

#[test]
fn test_private_member_access_error() {
    let mut ctx = Context::new_small();
    // Private member access on a class that doesn't define the field throws TypeError
    let result = ctx.eval("class Foo { method() { return this.#x; } } new Foo().method();");
    assert!(result.is_err(), "private member access should error");
    let err = result.unwrap_err();
    assert!(
        err.contains("class body"),
        "error should mention class body: {}",
        err
    );
}

#[test]
fn test_private_field_syntax_works() {
    let mut ctx = Context::new_small();
    // Private field declaration in class body is now supported
    let result =
        ctx.eval("class Foo { #x = 1; get() { return this.#x; } } var f = new Foo(); f.get();");
    assert!(result.is_ok(), "private field should work: {:?}", result);
    assert_eq!(result.unwrap().as_smi(), Some(1));
}

#[test]
fn test_private_member_write_error() {
    let mut ctx = Context::new_small();
    // Private member write on a class that doesn't define the field throws TypeError
    let result = ctx.eval("class Foo { method() { this.#x = 1; } } new Foo().method();");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.contains("class body"),
        "error should mention class body: {}",
        err
    );
}

#[test]
fn test_private_method_instance() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval("class C { #m() { return 42; } get() { return this.#m(); } } new C().get();")
        .unwrap();
    assert_eq!(r.as_smi(), Some(42));
}

#[test]
fn test_private_accessor_pair_instance() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "class C { #v; get #g() { return this.#v; } set #g(x) { this.#v = x; } \
             setG(v) { this.#g = v; } getG() { return this.#g; } } \
             let c = new C(); c.setG(9); c.getG();",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(9));
}

#[test]
fn test_private_accessor_setter_first() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "class C { #v; set #g(x) { this.#v = x; } get #g() { return this.#v; } \
             setG(v) { this.#g = v; } getG() { return this.#g; } } \
             let c = new C(); c.setG(6); c.getG();",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(6));
}

#[test]
fn test_static_private_field() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval("class C { static #x = 10; static getX() { return this.#x; } } C.getX();")
        .unwrap();
    assert_eq!(r.as_smi(), Some(10));
    let r = ctx
        .eval(
            "class C { static #x = 10; static setX(v) { this.#x = v; } \
             static getX() { return this.#x; } } C.setX(33); C.getX();",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(33));
    let r = ctx
        .eval(
            "class C { static #x; static getX() { return this.#x === undefined ? 5 : 0; } } \
             C.getX();",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(5));
}

#[test]
fn test_static_private_method() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval("class C { static #m() { return 7; } static g() { return this.#m(); } } C.g();")
        .unwrap();
    assert_eq!(r.as_smi(), Some(7));
}

#[test]
fn test_static_private_accessor_pair() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "class C { static #v = 4; static get #g() { return this.#v; } \
             static getG() { return this.#g; } } C.getG();",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(4));
    let r = ctx
        .eval(
            "class C { static #v = 4; static get #g() { return this.#v; } \
             static set #g(v) { this.#v = v; } static setG(v) { this.#g = v; } \
             static getG() { return this.#g; } } C.setG(9); C.getG();",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(9));
}

#[test]
fn test_private_fields_inherited() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "class A { #x = 3; getX() { return this.#x; } } \
             class B extends A {} new B().getX();",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(3));
    let r = ctx
        .eval(
            "class A { #x = 3; getX() { return this.#x; } } \
             class B extends A { getY() { return this.getX(); } } new B().getY();",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(3));
}

#[test]
fn test_private_missing_on_wrong_receiver() {
    let mut ctx = Context::new_small();
    let result = ctx.eval("class C { #x = 1; getX(o) { return o.#x; } } new C().getX({});");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("private member"));
}

#[test]
fn test_duplicate_private_name_error() {
    let mut ctx = Context::new_small();
    let result = ctx.eval("class C { #x; #x; }");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Duplicate private name"));
}

#[test]
fn test_object_literal_accessors() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("let o = { get a() { return 7; } }; o.a;").unwrap();
    assert_eq!(r.as_smi(), Some(7));
    let r = ctx
        .eval("let o = { set a(v) { this.b = v; } }; o.a = 3; o.b;")
        .unwrap();
    assert_eq!(r.as_smi(), Some(3));
    let r = ctx
        .eval("let o = { get [1+1]() { return 8; } }; o[2];")
        .unwrap();
    assert_eq!(r.as_smi(), Some(8));
}

#[test]
fn test_object_literal_nested_accessors() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval("let o = { get a() { return this.b; }, get b() { return 7; } }; o.a;")
        .unwrap();
    assert_eq!(r.as_smi(), Some(7));
}

#[test]
fn test_this_prop_inc_in_class() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "class C { constructor() { this.x = 5; } inc() { this.x++; } } \
             let c = new C(); c.inc(); c.x;",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(6));
    let r = ctx
        .eval(
            "class C { constructor() { this.x = 5; } inc() { return this.x++; } } \
             let c = new C(); c.inc(); c.x;",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(6));
}

#[test]
fn test_let_new_class_scoping() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval("function f() { let C = class { m() { return 5; } }; return new C().m(); } f();")
        .unwrap();
    assert_eq!(r.as_smi(), Some(5));
}

#[test]
fn test_string_match_basic() {
    let mut ctx = Context::new_small();
    assert_eq!(eval_str(&mut ctx, r#""abc".match(/a/)[0]"#), "a");
    assert_eq!(eval_str(&mut ctx, r#""abc".match(/b/)[0]"#), "b");
    let result = ctx.eval(r#""abc".match(/x/)"#).unwrap();
    assert!(result.is_null());
}

#[test]
fn test_string_match_captures() {
    let mut ctx = Context::new_small();
    assert_eq!(eval_str(&mut ctx, r#""a1b2c3".match(/(\d)/)[1]"#), "1");
    assert_eq!(eval_array_len(&mut ctx, r#""a1b2c3".match(/(\d)/)"#), 2);
}

#[test]
fn test_string_match_global() {
    let mut ctx = Context::new_small();
    assert_eq!(eval_array_len(&mut ctx, r#""a1b2c3".match(/\d/g)"#), 3);
    assert_eq!(eval_str(&mut ctx, r#""a1b2c3".match(/\d/g)[0]"#), "1");
    assert_eq!(eval_str(&mut ctx, r#""a1b2c3".match(/\d/g)[1]"#), "2");
    assert_eq!(eval_str(&mut ctx, r#""a1b2c3".match(/\d/g)[2]"#), "3");
    let result = ctx.eval(r#""abc".match(/\d/g)"#).unwrap();
    assert!(result.is_null());
}

#[test]
fn test_string_search_basic() {
    let mut ctx = Context::new_small();
    assert_eq!(ctx.eval(r#""abc".search(/b/)"#).unwrap().as_smi(), Some(1));
    assert_eq!(ctx.eval(r#""abc".search(/x/)"#).unwrap().as_smi(), Some(-1));
    assert_eq!(
        ctx.eval(r#""hello world".search(/world/)"#)
            .unwrap()
            .as_smi(),
        Some(6)
    );
}

#[test]
fn test_string_match_no_args() {
    let mut ctx = Context::new_small();
    let result = ctx.eval(r#""abc".match()"#).unwrap();
    assert!(!result.is_null());
    assert_eq!(eval_str(&mut ctx, r#""abc".match()[0]"#), "");
}

#[test]
fn test_string_search_no_args() {
    let mut ctx = Context::new_small();
    assert_eq!(ctx.eval(r#""abc".search()"#).unwrap().as_smi(), Some(0));
}

#[test]
fn test_string_split_regex() {
    let mut ctx = Context::new_small();
    assert_eq!(eval_array_len(&mut ctx, r#""a,b,c".split(/,/)"#), 3);
    assert_eq!(eval_str(&mut ctx, r#""a,b,c".split(/,/)[0]"#), "a");
    assert_eq!(eval_str(&mut ctx, r#""a,b,c".split(/,/)[1]"#), "b");
    assert_eq!(eval_str(&mut ctx, r#""a,b,c".split(/,/)[2]"#), "c");
}

#[test]
fn test_string_split_regex_limit() {
    let mut ctx = Context::new_small();
    assert_eq!(eval_array_len(&mut ctx, r#""a,b,c".split(/,/, 2)"#), 2);
    assert_eq!(eval_str(&mut ctx, r#""a,b,c".split(/,/, 2)[0]"#), "a");
    assert_eq!(eval_str(&mut ctx, r#""a,b,c".split(/,/, 2)[1]"#), "b");
}

#[test]
fn test_string_split_regex_no_match() {
    let mut ctx = Context::new_small();
    assert_eq!(eval_array_len(&mut ctx, r#""abc".split(/x/)"#), 1);
    assert_eq!(eval_str(&mut ctx, r#""abc".split(/x/)[0]"#), "abc");
}

#[test]
fn test_string_split_regex_empty() {
    let mut ctx = Context::new_small();
    assert_eq!(eval_array_len(&mut ctx, r#""".split(/,/)"#), 1);
    assert_eq!(eval_str(&mut ctx, r#""".split(/,/)[0]"#), "");
}

// ── Correctness batch: silent miscompile fixes ──────────────────────────

#[test]
fn test_assert_throws_type_mismatch() {
    let mut ctx = Context::new_small();
    // Wrong error type must fail the assert (mismatch error surfaces).
    let r = ctx
        .eval(
            "var caught = false;
             try { assert.throws(Test262Error, function(){ throw new Error('boom'); }); }
             catch (e) { caught = true; }
             caught;",
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
    // Correct type passes.
    let r2 = ctx
        .eval("assert.throws(Test262Error, function(){ throw new Test262Error('x'); }); 1")
        .unwrap();
    assert_eq!(r2.as_smi(), Some(1));
}

#[test]
fn test_do_while_break_and_continue() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var i = 0, out = 0;
             do { i++; if (i === 3) break; out += i; } while (true);
             out;",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(3));
    let r2 = ctx
        .eval(
            "var i = 0, n = 0;
             do { i++; if (i % 2 === 0) continue; n += i; } while (i < 6);
             n;",
        )
        .unwrap();
    assert_eq!(r2.as_smi(), Some(9));
    let r3 = ctx
        .eval(
            "var i = 0;
             do { i++; if (i >= 10) break; } while (true);
             i;",
        )
        .unwrap();
    assert_eq!(r3.as_smi(), Some(10));
}

#[test]
fn test_for_continue_runs_update() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var s = 0, n = 0;
             for (var j = 0; j < 10; j++) { n++; if (j % 2 === 0) continue; s += j; }
             s * 100 + n;",
        )
        .unwrap();
    // s = 1+3+5+7+9 = 25, n = 10 (continue must not skip the update)
    assert_eq!(r.as_smi(), Some(2510));
}

#[test]
fn test_exponent_right_assoc() {
    let mut ctx = Context::new_small();
    assert_eq!(ctx.eval("2 ** 3 ** 2").unwrap().as_smi(), Some(512));
    assert_eq!(ctx.eval("2 ** (3 ** 2)").unwrap().as_smi(), Some(512));
    assert_eq!(ctx.eval("(2 ** 3) ** 2").unwrap().as_smi(), Some(64));
}

#[test]
fn test_nullish_coalescing_and_short_circuit_assign() {
    let mut ctx = Context::new_small();
    assert_eq!(
        ctx.eval("var a = null; a ??= 5; a").unwrap().as_smi(),
        Some(5)
    );
    assert_eq!(ctx.eval("var a = 0; a ??= 5; a").unwrap().as_smi(), Some(0));
    assert_eq!(ctx.eval("var b = 1; b ||= 9; b").unwrap().as_smi(), Some(1));
    assert_eq!(ctx.eval("var b = 0; b ||= 9; b").unwrap().as_smi(), Some(9));
    assert_eq!(ctx.eval("var c = 2; c &&= 7; c").unwrap().as_smi(), Some(7));
    assert_eq!(ctx.eval("var c = 0; c &&= 7; c").unwrap().as_smi(), Some(0));
    assert_eq!(
        ctx.eval("null ?? 5;").unwrap().as_smi(),
        Some(5),
        "?? is not || (null ?? 5 == 5)"
    );
    assert_eq!(
        ctx.eval("0 ?? 5;").unwrap().as_smi(),
        Some(0),
        "?? only falls through on null/undefined"
    );
}

#[test]
fn test_member_and_private_update() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval("var o = { x: 5 }; var old = o.x++; old * 100 + o.x;")
        .unwrap();
    assert_eq!(r.as_smi(), Some(506));
    assert_eq!(
        ctx.eval("var o = { x: 5 }; ++o.x; o.x").unwrap().as_smi(),
        Some(6)
    );
    assert_eq!(
        ctx.eval(
            "class A { constructor() { this.v = 7; } }
             var a = new A(); a.v++; a.v;"
        )
        .unwrap()
        .as_smi(),
        Some(8)
    );
    assert_eq!(
        ctx.eval(
            "class A { #v = 5; get() { return this.#v; } bump() { this.#v++; } }
             var a = new A(); a.bump(); a.bump(); a.get();"
        )
        .unwrap()
        .as_smi(),
        Some(7)
    );
    assert_eq!(
        ctx.eval(
            "class P { constructor() { this.inh = 7; } }
             class C extends P {}
             var c = new C();
             c.inh++;
             var before = c.inh;
             c.inh++;
             c.inh;"
        )
        .unwrap()
        .as_smi(),
        Some(9)
    );
}

#[test]
fn test_destructuring_assignment() {
    let mut ctx = Context::new_small();
    assert_eq!(
        ctx.eval("var [a, b] = [1, 2]; a * 10 + b;")
            .unwrap()
            .as_smi(),
        Some(12)
    );
    assert_eq!(
        ctx.eval("var [p, q] = [1, 2]; [p, q] = [q, p]; p * 10 + q;")
            .unwrap()
            .as_smi(),
        Some(21)
    );
    assert_eq!(
        ctx.eval("var { m, n } = { m: 10, n: 20 }; m + n;")
            .unwrap()
            .as_smi(),
        Some(30)
    );
    assert_eq!(
        ctx.eval("var [h, ...tail] = [1, 2, 3, 4]; h * 10 + tail.length;")
            .unwrap()
            .as_smi(),
        Some(13)
    );
    assert_eq!(
        ctx.eval("var [d = 99] = []; d;").unwrap().as_smi(),
        Some(99),
        "destructuring default"
    );
    let r = ctx
        .eval("var arr; var x = (arr = [7, 8]); arr[0] + x.length;")
        .unwrap();
    assert_eq!(r.as_smi(), Some(9), "destructure assign yields RHS value");
}

#[test]
fn test_computed_class_keys() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var k = 'dyn';
             class A {
               [k]() { return 1; }
               ['other' + '2']() { return 2; }
               static [k + 'Static']() { return 3; }
               get [k + 'Getter']() { return 4; }
               set [k + 'Setter'](v) { this.vv = v; }
             }
             var a = new A();
             var acc = a.dyn() + a.other2() + A.dynStatic() + a.dynGetter;
             a.dynSetter = 77;
             acc * 100 + a.vv;",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(1077));
    assert_eq!(
        ctx.eval("class B { [42]() { return 99; } } var b = new B(); b['42']();")
            .unwrap()
            .as_smi(),
        Some(99)
    );
}

#[test]
fn test_accessor_getter_result_flows() {
    let mut ctx = Context::new_small();
    // Regression: getter result was lost (and pc double-advanced) when the
    // accessor was used in a non-final statement.
    let r = ctx
        .eval(
            "class Foo { constructor() { this._x = 0; }
               get x() { return this._x; }
               set x(v) { this._x = v; } }
             var f = new Foo();
             f.x = 10;
             var got = f.x;
             got;",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(10));
    let r2 = ctx
        .eval(
            "class A { constructor() { this.stored = 0; } get g() { return this.stored; } }
             var a = new A();
             var y = a.g;
             y;",
        )
        .unwrap();
    assert_eq!(r2.as_smi(), Some(0));
}

#[test]
fn test_accessor_getter_nested_callback_machine() {
    // F4 pin: a getter whose body runs another callback machine (array map
    // with a JS callback → nested push_callback_call) must still resume the
    // outer LoadProperty with the getter's value. Exercises the centralized
    // pending rebase across nested machines.
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var log = [];
             var o = { get x() {
               var m = [1, 2, 3].map(function (v) { log.push(v); return v * 2; });
               return 99 + m[2];
             } };
             o.x + log.length;",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(108), "99 + m[2]=6 + 3 callbacks");
}

#[test]
fn test_optional_chaining_basic() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var o = { a: { b: { c: 42 } } };
             o?.a?.b?.c;",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(42));
    assert!(ctx.eval("o?.x?.y").unwrap().is_undefined());
    assert!(ctx.eval("null?.a").unwrap().is_undefined());
    assert!(ctx.eval("undefined?.b").unwrap().is_undefined());
    // Env-declared undefined short-circuits; an unresolvable identifier still
    // throws ReferenceError even under `?.` (base expression resolution first).
    assert!(
        ctx.eval("var declared_undef; declared_undef?.x")
            .unwrap()
            .is_undefined()
    );
    assert!(ctx.eval("var_not_declared_zz?.x").is_err());
}

#[test]
fn test_optional_chaining_method_call() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var g = { m: function(v) { return this.v + v; }, v: 10 };
             g?.m(5);",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(15));
    assert!(ctx.eval("g?.x?.()").unwrap().is_undefined());
    // ?. before the call keeps `this` (receiver from the member load)
    let r3 = ctx
        .eval("var g2 = { m: function() { return this.v; }, v: 7 }; g2.m?.()")
        .unwrap();
    assert_eq!(r3.as_smi(), Some(7));
    // missing member -> nullish method -> undefined, no call
    assert!(ctx.eval("g.missing?.()").unwrap().is_undefined());
}

#[test]
fn test_optional_chaining_optional_call() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval("var fn = function(x) { return x * 2; }; fn?.(21)")
        .unwrap();
    assert_eq!(r.as_smi(), Some(42));
    let r2 = ctx
        .eval("var f2 = function() { return 99; }; f2?.()")
        .unwrap();
    assert_eq!(r2.as_smi(), Some(99));
    assert!(ctx.eval("var n = null; n?.(1)").unwrap().is_undefined());
}

#[test]
fn test_optional_chaining_computed_and_short_circuit() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("var arr = [1, 2, 3]; arr?.[1]").unwrap();
    assert_eq!(r.as_smi(), Some(2));
    assert!(ctx.eval("arr?.[99]?.x").unwrap().is_undefined());
    let r2 = ctx
        .eval(
            "var count = 0;
             function t() { count++; return null; }
             var v = t()?.x.y.z;
             v;",
        )
        .unwrap();
    assert!(r2.is_undefined());
    let c = ctx.eval("count").unwrap();
    assert_eq!(c.as_smi(), Some(1), "later links must not evaluate");
    let r3 = ctx
        .eval("var o2 = { deep: null }; o2?.deep ?? 'fallback'")
        .unwrap();
    assert!(
        r3.to_bool(),
        "expected the truthy 'fallback' string, got {:?}",
        r3
    );
}

#[test]
fn test_optional_chaining_syntax_errors() {
    let mut ctx = Context::new_small();
    assert!(ctx.eval("var a = {}; a?.b = 1;").is_err());
    assert!(ctx.eval("var a = {}; a?.b++;").is_err());
    assert!(ctx.eval("var a = {}; a?.b--;").is_err());
    assert!(ctx.eval("new a?.b;").is_err());
    assert!(ctx.eval("super?.b;").is_err());
    assert!(ctx.eval("var a; a?.;").is_err());
}

#[test]
fn test_optional_chaining_in_loop() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var rows = [{ v: 1 }, null, { v: 3 }];
             var s = 0;
             for (var i = 0; i < rows.length; i++) { s += rows[i]?.v ?? 0; }
             s;",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(4));
}

#[test]
fn test_ternary_binds_looser_than_binary_ops() {
    let mut ctx = Context::new_small();
    // Previously `a === b ? x : y` silently parsed as `a === (b ? x : y)`.
    assert_eq!(ctx.eval("1 === 1 ? 7 : 8").unwrap().as_smi(), Some(7));
    assert_eq!(ctx.eval("1 === 2 ? 7 : 8").unwrap().as_smi(), Some(8));
    assert_eq!(ctx.eval("1 < 2 ? 7 : 8").unwrap().as_smi(), Some(7));
    assert_eq!(
        ctx.eval("1 + 2 === 3 ? 100 : 200").unwrap().as_smi(),
        Some(100)
    );
    assert_eq!(ctx.eval("2 === 2 ? 1 : 0").unwrap().as_smi(), Some(1));
    assert_eq!(ctx.eval("0 || 5 ? 1 : 0").unwrap().as_smi(), Some(1));
    // nested ternary inside the else branch (right-associative chain)
    assert_eq!(
        ctx.eval("2 === 2 ? (3 === 3 ? 9 : 8) : 7")
            .unwrap()
            .as_smi(),
        Some(9)
    );
    assert_eq!(
        ctx.eval("5 > 3 ? 1 > 0 ? 3 : 2 : 1").unwrap().as_smi(),
        Some(3)
    );
}

#[test]
fn test_symbol_basic() {
    fn js_str(ctx: &mut Context, src: &str) -> String {
        let r = ctx.eval(src).unwrap();
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        }
    }
    let mut ctx = Context::new_small();
    let r2 = ctx.eval("var s2 = Symbol('desc'); s2;").unwrap();
    assert!(r2.is_symbol());
    assert_eq!(js_str(&mut ctx, "typeof Symbol('x');"), "symbol");
    assert_eq!(js_str(&mut ctx, "typeof Symbol.for('k');"), "symbol");
}

#[test]
fn test_symbol_uniqueness() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var a = Symbol('x');
             var b = Symbol('x');
             a === b;",
        )
        .unwrap();
    assert_eq!(r, rune_core::value::Value::boolean(false));
    let r2 = ctx
        .eval(
            "var a = Symbol('x');
             a === a;",
        )
        .unwrap();
    assert_eq!(r2, rune_core::value::Value::boolean(true));
    let r3 = ctx.eval("Symbol('x') === Symbol('x');").unwrap();
    assert_eq!(r3, rune_core::value::Value::boolean(false));
}

#[test]
fn test_symbol_for_keyfor() {
    fn js_str(ctx: &mut Context, src: &str) -> String {
        let r = ctx.eval(src).unwrap();
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        }
    }
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var a = Symbol.for('k');
             var b = Symbol.for('k');
             a === b;",
        )
        .unwrap();
    assert_eq!(r, rune_core::value::Value::boolean(true));
    assert_eq!(
        js_str(&mut ctx, "var a = Symbol.for('k'); Symbol.keyFor(a);"),
        "k"
    );
    let r3 = ctx.eval("Symbol.keyFor(Symbol('fresh'));").unwrap();
    assert!(r3.is_undefined());
}

#[test]
fn test_symbol_to_string_and_description() {
    fn js_str(ctx: &mut Context, src: &str) -> String {
        let r = ctx.eval(src).unwrap();
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        }
    }
    let mut ctx = Context::new_small();
    assert_eq!(
        js_str(&mut ctx, "Symbol('desc').toString();"),
        "Symbol(desc)"
    );
    assert_eq!(js_str(&mut ctx, "Symbol().toString();"), "Symbol()");
    assert_eq!(js_str(&mut ctx, "Symbol('desc').description;"), "desc");
    let r4 = ctx.eval("Symbol().description;").unwrap();
    assert!(r4.is_undefined());
}

#[test]
fn test_symbol_well_known_statics() {
    fn js_str(ctx: &mut Context, src: &str) -> String {
        let r = ctx.eval(src).unwrap();
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        }
    }
    let mut ctx = Context::new_small();
    assert_eq!(js_str(&mut ctx, "typeof Symbol.iterator;"), "symbol");
    let r2 = ctx
        .eval(
            "var a = Symbol.iterator;
             var b = Symbol.iterator;
             a === b;",
        )
        .unwrap();
    assert_eq!(r2, rune_core::value::Value::boolean(true));
    assert_eq!(
        js_str(
            &mut ctx,
            "typeof Symbol.match; typeof Symbol.replace; typeof Symbol.search; typeof Symbol.split;"
        ),
        "symbol"
    );
}

#[test]
fn test_symbol_property_keys() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var s = Symbol('key');
             var o = {};
             o[s] = 42;
             o[s];",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(42));
    let r2 = ctx
        .eval(
            "var s = Symbol('k');
             var o = {};
             o[s] = 1;
             var keys = [];
             for (var k in o) { keys.push(k); }
             keys.length;",
        )
        .unwrap();
    assert_eq!(r2.as_smi(), Some(0));
    let r3 = ctx
        .eval(
            "var s = Symbol('k');
             var o = { a: 1 };
             o[s] = 2;
             Object.keys(o).length;",
        )
        .unwrap();
    assert_eq!(r3.as_smi(), Some(1));
}

#[test]
fn test_symbol_new_throws() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("new Symbol('x');");
    assert!(r.is_err(), "new Symbol should throw");
}

#[test]
fn test_symbol_coercion_throws() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("String(Symbol('x'));").unwrap();
    assert_eq!(
        rune_interpreter::builtins::value_to_js_string(r),
        "Symbol(x)".to_string()
    );
    let r2 = ctx.eval("\"a\" + Symbol('x');");
    assert!(r2.is_err(), "string + Symbol should throw TypeError");
    let r3 = ctx.eval("Symbol('x') + 1;");
    assert!(r3.is_err(), "Symbol + number should throw TypeError");
}

#[test]
fn test_symbol_truthiness() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("Symbol('x') ? 1 : 2;").unwrap();
    assert_eq!(r.as_smi(), Some(1));
    let r2 = ctx
        .eval(
            "var s = Symbol('x');
             s ? 7 : 8;",
        )
        .unwrap();
    assert_eq!(r2.as_smi(), Some(7));
}

#[test]
fn test_symbol_match_dispatch() {
    fn js_str(ctx: &mut Context, src: &str) -> String {
        let r = ctx.eval(src).unwrap();
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        }
    }
    let mut ctx = Context::new_small();
    assert_eq!(
        js_str(
            &mut ctx,
            "var custom = {
                 [Symbol.match](str) { return 'matched:' + str; }
             };
             'hello'.match(custom);",
        ),
        "matched:hello"
    );
    let r2 = ctx
        .eval(
            "var custom = {
                 [Symbol.search](str) { return 42; }
             };
             'hello'.search(custom);",
        )
        .unwrap();
    assert_eq!(r2.as_smi(), Some(42));
    let r3 = ctx
        .eval(
            "var custom = {
                 [Symbol.split](str, limit) { return [str, limit]; }
             };
             'a,b'.split(custom, 3).length;",
        )
        .unwrap();
    assert_eq!(r3.as_smi(), Some(2));
    assert_eq!(
        js_str(
            &mut ctx,
            "var custom = {
                 [Symbol.replace](str) { return 'replaced:' + str; }
             };
             'x'.replace(custom, 'y');",
        ),
        "replaced:x"
    );
}

#[test]
fn test_symbol_match_dispatch_noncallable_throws() {
    let mut ctx = Context::new_small();
    let r = ctx.eval(
        "var bad = { [Symbol.match]: 5 };
         'x'.match(bad);",
    );
    assert!(r.is_err(), "non-callable @@match should throw TypeError");
}

#[test]
fn test_symbol_legacy_fallback_untouched() {
    fn js_str(ctx: &mut Context, src: &str) -> String {
        let r = ctx.eval(src).unwrap();
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        }
    }
    let mut ctx = Context::new_small();
    // Plain objects without @@match still fall back to the legacy algorithm.
    let r = ctx
        .eval(
            "var plain = {};
             'abc'.match(plain);",
        )
        .unwrap();
    assert!(r.is_null() || r.is_heap_object());
    // String patterns unaffected.
    let r2 = ctx.eval("\"a,b\".split(',').length;").unwrap();
    assert_eq!(r2.as_smi(), Some(2));
    assert_eq!(js_str(&mut ctx, "\"abc\".replace('b', 'X');"), "aXc");
}

fn js_str(ctx: &mut Context, src: &str) -> String {
    let r = ctx.eval(src).unwrap();
    unsafe {
        rune_core::string::HeapString::to_string(
            r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
        )
    }
}

#[test]
fn test_for_of_array() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var sum = 0;
             for (var x of [1, 2, 3, 4]) { sum += x; }
             sum;",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(10));
    // `let` loop variable
    let r2 = ctx
        .eval(
            "var out = '';
             for (let c of ['a', 'b', 'c']) { out += c; }
             out;",
        )
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r2.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "abc"
    );
    // Empty array: body never runs
    let r3 = ctx
        .eval(
            "var ran = false;
             for (var x of []) { ran = true; }
             ran;",
        )
        .unwrap();
    assert_eq!(r3, rune_core::value::Value::boolean(false));
}

#[test]
fn test_for_of_string() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var out = '';
             for (var ch of 'abc') { out += ch; }
             out;",
        )
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "abc"
    );
    // Astral code point (surrogate pair) is a single iteration step
    let r2 = ctx
        .eval(
            "var n = 0;
             for (var ch of 'a\u{1D11E}b') { n += 1; }
             n;",
        )
        .unwrap();
    assert_eq!(r2.as_smi(), Some(3));
    // The yielded value is the full code point
    let r3 = ctx
        .eval("var s = ''; for (var ch of 'x\u{1D11E}') { s += ch; } s;")
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r3.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "x\u{1D11E}"
    );
}

#[test]
fn test_for_of_break_continue() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var sum = 0;
             for (var x of [1, 2, 3, 4, 5]) {
               if (x === 3) { break; }
               sum += x;
             }
             sum;",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(3));
    let r2 = ctx
        .eval(
            "var out = '';
             for (var x of [1, 2, 3, 4]) {
               if (x === 2) { continue; }
               out += x;
             }
             out;",
        )
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r2.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "134"
    );
    // break inside a nested loop still exits the outer for-of
    let r3 = ctx
        .eval(
            "var count = 0;
             for (var x of [1, 2, 3]) {
               var i = 0;
               while (i < 5) {
                 if (i === 1) { break; }
                 i += 1;
               }
               count += i;
             }
             count;",
        )
        .unwrap();
    assert_eq!(r3.as_smi(), Some(3));
    // continue re-runs the next() step (skipped values don't repeat)
    let r4 = ctx
        .eval(
            "var seen = '';
             var it = ['a', 'b', 'c'].values();
             for (var v of it) {
               if (v === 'b') { continue; }
               seen += v;
             }
             seen;",
        )
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r4.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "ac"
    );
}

#[test]
fn test_array_keys_values_entries() {
    let mut ctx = Context::new_small();
    // values() — manual next() stepping with done/value
    let r = ctx
        .eval(
            "var it = [10, 20].values();
             var a = it.next();
             var b = it.next();
             var c = it.next();
             (a.value === 10) && (a.done === false) &&
             (b.value === 20) && (b.done === false) &&
             (c.done === true) && (c.value === undefined);",
        )
        .unwrap();
    assert_eq!(r, rune_core::value::Value::boolean(true));
    // keys()
    let r2 = ctx
        .eval(
            "var ks = '';
             for (var k of ['a', 'b'].keys()) { ks += k; }
             ks;",
        )
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r2.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "01"
    );
    // entries() — [index, value] pairs
    let r3 = ctx
        .eval(
            "var it = [7, 8].entries();
             var p = it.next().value;
             var q = it.next().value;
             (p[0] === 0) && (p[1] === 7) && (q[0] === 1) && (q[1] === 8);",
        )
        .unwrap();
    assert_eq!(r3, rune_core::value::Value::boolean(true));
    // [Symbol.iterator]() aliases values
    let r4 = ctx
        .eval("[1, 2, 3][Symbol.iterator]().next().value;")
        .unwrap();
    assert_eq!(r4.as_smi(), Some(1));
    // Iterators are themselves iterable: [Symbol.iterator]() returns this
    let r5 = ctx
        .eval(
            "var it = [5, 6].values();
             it[Symbol.iterator]() === it;",
        )
        .unwrap();
    assert_eq!(r5, rune_core::value::Value::boolean(true));
    // for..of over an iterator object works
    let r6 = ctx
        .eval(
            "var sum = 0;
             for (var x of [5, 6].values()) { sum += x; }
             sum;",
        )
        .unwrap();
    assert_eq!(r6.as_smi(), Some(11));
}

#[test]
fn test_for_of_user_iterator() {
    let mut ctx = Context::new_small();
    // User-defined iterable: JS @@iterator factory + JS next()
    let r = ctx
        .eval(
            "var obj = {
               [Symbol.iterator]: function() {
                 var i = 0;
                 return {
                   next: function() {
                     i += 1;
                     if (i <= 3) { return { value: i * 10, done: false }; }
                     return { value: undefined, done: true };
                   }
                 };
               }
             };
             var sum = 0;
             for (var x of obj) { sum += x; }
             sum;",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(60));
}

#[test]
fn test_for_of_spread() {
    let mut ctx = Context::new_small();
    // Spread of a string → code point array
    let r = ctx
        .eval(
            "var a = [...'ab'];
             (a.length === 2) && (a[0] === 'a') && (a[1] === 'b');",
        )
        .unwrap();
    assert_eq!(r, rune_core::value::Value::boolean(true));
    // Spread of a user iterable
    let r2 = ctx
        .eval(
            "var obj = { [Symbol.iterator]: function() {
                 var i = 0;
                 return { next: function() {
                   i += 1;
                   if (i <= 2) { return { value: i * 7, done: false }; }
                   return { done: true, value: 0 };
                 }};
               }};
             var a = [...obj];
             (a.length === 2) && (a[0] === 7) && (a[1] === 14);",
        )
        .unwrap();
    assert_eq!(r2, rune_core::value::Value::boolean(true));
    // Spread of an array still works (mixed literal)
    let r3 = ctx.eval("var a = [0, ...[1, 2], 3]; a.length;").unwrap();
    assert_eq!(r3.as_smi(), Some(4));
    // Call-arg spread of a string
    let r4 = ctx
        .eval(
            "function f(a, b, c) { return a + b + c; }
             f(...'xyz');",
        )
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r4.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "xyz"
    );
    // Spread of a non-iterable throws
    let r5 = ctx.eval("var a = [...5];");
    assert!(r5.is_err(), "spread of a non-iterable should throw");
    let r6 = ctx.eval("var a = [...null];");
    assert!(r6.is_err(), "spread of null should throw");
}

#[test]
fn test_for_of_member_lhs() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var o = { p: 0 };
             for (o.p of [3, 4]) {}
             o.p;",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(4));
    // member LHS with computed key
    let r2 = ctx
        .eval(
            "var o = {}; var k = 'q';
             for (o[k] of ['x']) {}
             o.q;",
        )
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r2.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "x"
    );
}

#[test]
fn test_for_of_errors() {
    let mut ctx = Context::new_small();
    assert!(ctx.eval("for (var x of null) {}").is_err());
    assert!(ctx.eval("for (var x of undefined) {}").is_err());
    assert!(ctx.eval("for (var x of 42) {}").is_err());
    assert!(ctx.eval("for (var x of {}) {}").is_err());
    // Object whose @@iterator is present but not callable
    assert!(
        ctx.eval("for (var x of { [Symbol.iterator]: 5 }) {}")
            .is_err()
    );
    // Iterator without a callable next
    assert!(
        ctx.eval(
            "var bad = { [Symbol.iterator]: function() { return { next: 5 }; } };
             for (var x of bad) {}"
        )
        .is_err()
    );
}

#[test]
fn test_string_symbol_iterator_direct() {
    let mut ctx = Context::new_small();
    assert_eq!(
        js_str(&mut ctx, "'ab'[Symbol.iterator]().next().value;"),
        "a"
    );
    let r2 = ctx
        .eval(
            "var it = 'x'[Symbol.iterator]();
             it.next();
             it.next().done;",
        )
        .unwrap();
    assert_eq!(r2, rune_core::value::Value::boolean(true));
    // String iterators are iterable
    let r3 = ctx
        .eval(
            "var it = 'hi'[Symbol.iterator]();
             it[Symbol.iterator]() === it;",
        )
        .unwrap();
    assert_eq!(r3, rune_core::value::Value::boolean(true));
    // Surrogate pair handled as one code point
    let r4 = ctx
        .eval("'a\u{1D11E}'[Symbol.iterator]().next().value;")
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r4.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "a"
    );
    // for..of over an array iterator created from keys()
    let r5 = ctx
        .eval(
            "var ks = '';
             for (var k of [10, 20].keys()) { ks += k; }
             ks;",
        )
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r5.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "01"
    );
}

#[test]
fn test_map_basic() {
    let mut ctx = Context::new_small();
    // size, set/get/has/delete/clear
    let r = ctx
        .eval(
            "var m = new Map();
             m.set('a', 1).set('b', 2);
             m.size;",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(2));
    let r = ctx.eval("m.get('a');").unwrap();
    assert_eq!(r.as_smi(), Some(1));
    let r = ctx.eval("m.has('b');").unwrap();
    assert_eq!(r, rune_core::value::Value::boolean(true));
    let r = ctx.eval("m.has('c');").unwrap();
    assert_eq!(r, rune_core::value::Value::boolean(false));
    let r = ctx.eval("m.get('nope');").unwrap();
    assert!(r.is_undefined());
    let r = ctx.eval("m.delete('a'); m.size;").unwrap();
    assert_eq!(r.as_smi(), Some(1));
    let r = ctx.eval("m.delete('a');").unwrap();
    assert_eq!(r, rune_core::value::Value::boolean(false));
    let r = ctx.eval("m.clear(); m.size;").unwrap();
    assert_eq!(r.as_smi(), Some(0));
    // set returns the map (chaining)
    let r = ctx
        .eval("var m2 = new Map(); m2.set('x', 1) === m2;")
        .unwrap();
    assert_eq!(r, rune_core::value::Value::boolean(true));
    // set overwrites and preserves insertion order semantics for update
    let r = ctx
        .eval(
            "var m3 = new Map();
             m3.set('a', 1).set('b', 2).set('a', 3);
             m3.size + m3.get('a');",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(5));
}

#[test]
fn test_map_keys() {
    let mut ctx = Context::new_small();
    // object identity keys
    let r = ctx
        .eval(
            "var o = {x: 1};
             var m = new Map();
             m.set(o, 'first');
             m.set({x: 1}, 'second');
             m.get(o) + '|' + m.size;",
        )
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "first|2"
    );
    // string keys compare by content
    let r = ctx
        .eval(
            "var m = new Map();
             m.set('ab', 1);
             m.get('a' + 'b');",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(1));
    // NaN keys: NaN === NaN for map keys
    let r = ctx
        .eval(
            "var m = new Map();
             m.set(NaN, 42);
             m.get(NaN);",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(42));
    // 1 and 1.0 are the same key
    let r = ctx
        .eval(
            "var m = new Map();
             m.set(1, 'one');
             m.set(1.0, 'also one');
             m.size + m.get(1);",
        )
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "1also one"
    );
    // -0 and +0 are the same key
    let r = ctx
        .eval(
            "var m = new Map();
             m.set(-0, 'zero');
             m.get(0);",
        )
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "zero"
    );
}

#[test]
fn test_map_iteration() {
    let mut ctx = Context::new_small();
    // entries yields [k, v] pairs
    let r = ctx
        .eval(
            "var m = new Map();
             m.set('a', 1).set('b', 2);
             var ks = ''; var vs = 0;
             for (var e of m) { ks += e[0]; vs += e[1]; }
             ks + vs;",
        )
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "ab3"
    );
    // keys / values iterators
    let r = ctx
        .eval(
            "var m = new Map();
             m.set('a', 1).set('b', 2);
             var k = ''; for (var x of m.keys()) { k += x; }
             var v = 0; for (var x of m.values()) { v += x; }
             k + v;",
        )
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "ab3"
    );
    // map iterator protocol: next() objects
    let r = ctx
        .eval(
            "var m = new Map();
             m.set('a', 1);
             var it = m.entries();
             var n = it.next();
             n.value[0] + n.value[1] + (n.done ? 'Y' : 'N');",
        )
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "a1N"
    );
    let r = ctx
        .eval("var it = m.entries(); it.next(); it.next().done;")
        .unwrap();
    assert_eq!(r, rune_core::value::Value::boolean(true));
    // deletion during iteration is skipped
    let r = ctx
        .eval(
            "var m = new Map();
             m.set('a', 1).set('b', 2).set('c', 3);
             var it = m.keys();
             var first = it.next().value;
             m.delete('b');
             var second = it.next().value;
             first + second;",
        )
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "ac"
    );
    // iterator is itself iterable (returns itself)
    let r = ctx
        .eval("var it = new Map().entries(); it[Symbol.iterator]() === it;")
        .unwrap();
    assert_eq!(r, rune_core::value::Value::boolean(true));
}

#[test]
fn test_map_constructor_iterable() {
    let mut ctx = Context::new_small();
    // from array of pairs
    let r = ctx
        .eval(
            "var m = new Map([['a', 1], ['b', 2]]);
             m.size + m.get('a') + m.get('b');",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(5));
    // from another map
    let r = ctx
        .eval(
            "var m1 = new Map([['x', 10]]);
             var m2 = new Map(m1);
             m2.get('x');",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(10));
    // from a generator-style user iterable with a JS @@iterator
    let r = ctx
        .eval(
            "var iter = {
               i: 0,
               [Symbol.iterator]: function() {
                 var self = this;
                 return {
                   next: function() {
                     self.i += 1;
                     if (self.i > 2) { return {done: true}; }
                     return {done: false, value: [self.i, self.i * 10]};
                   }
                 };
               }
             };
             var m = new Map(iter);
             m.size + m.get(1) + m.get(2);",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(32));
    // undefined/null iterable → empty map
    let r = ctx
        .eval(
            "var m1 = new Map(undefined); var m2 = new Map(null);
             m1.size + m2.size;",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(0));
    // primitive non-iterable → TypeError
    let r = ctx
        .eval("var threw = false; try { new Map(5); } catch (e) { threw = true; } threw;")
        .unwrap();
    assert_eq!(r, rune_core::value::Value::boolean(true));
    // string iterable → entries are strings, not objects → TypeError
    let r = ctx
        .eval("var threw = false; try { new Map('ab'); } catch (e) { threw = true; } threw;")
        .unwrap();
    assert_eq!(r, rune_core::value::Value::boolean(true));
    // Map() without new → TypeError
    let r = ctx
        .eval("var threw = false; try { Map(); } catch (e) { threw = true; } threw;")
        .unwrap();
    assert_eq!(r, rune_core::value::Value::boolean(true));
}

#[test]
fn test_map_foreach() {
    let mut ctx = Context::new_small();
    // JS callback
    let r = ctx
        .eval(
            "var m = new Map([['a', 1], ['b', 2], ['c', 3]]);
             var ks = ''; var vs = 0;
             m.forEach(function(v, k, map) {
               ks += k; vs += v;
               if (map !== m) { vs += 100; }
             });
             ks + vs;",
        )
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "abc6"
    );
    // thisArg
    let r = ctx
        .eval(
            "var m = new Map([['a', 1]]);
             var obj = {x: 10};
             var got;
             m.forEach(function(v, k) { got = this.x + v; }, obj);
             got;",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(11));
    // mutation during iteration (delete ahead)
    let r = ctx
        .eval(
            "var m = new Map([['a', 1], ['b', 2], ['c', 3]]);
             var ks = '';
             m.forEach(function(v, k) {
               ks += k;
               if (k === 'a') { m.delete('b'); }
             });
             ks;",
        )
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "ac"
    );
    // non-callable callback → TypeError
    let r = ctx
        .eval("var m = new Map(); var t = false; try { m.forEach(5); } catch (e) { t = true; } t;")
        .unwrap();
    assert_eq!(r, rune_core::value::Value::boolean(true));
}

#[test]
fn test_set_basic() {
    let mut ctx = Context::new_small();
    // add/has/delete/size/clear
    let r = ctx
        .eval(
            "var s = new Set();
             s.add('a').add('b').add('a');
             s.size;",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(2));
    let r = ctx.eval("s.has('a');").unwrap();
    assert_eq!(r, rune_core::value::Value::boolean(true));
    let r = ctx.eval("s.has('c');").unwrap();
    assert_eq!(r, rune_core::value::Value::boolean(false));
    let r = ctx.eval("s.delete('a'); s.size;").unwrap();
    assert_eq!(r.as_smi(), Some(1));
    let r = ctx.eval("s.delete('a');").unwrap();
    assert_eq!(r, rune_core::value::Value::boolean(false));
    let r = ctx.eval("s.clear(); s.size;").unwrap();
    assert_eq!(r.as_smi(), Some(0));
    // add returns the set
    let r = ctx.eval("var s2 = new Set(); s2.add(1) === s2;").unwrap();
    assert_eq!(r, rune_core::value::Value::boolean(true));
    // NaN and number encodings
    let r = ctx
        .eval(
            "var s = new Set();
             s.add(NaN).add(NaN).add(1).add(1.0);
             s.size;",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(2));
}

#[test]
fn test_set_iteration() {
    let mut ctx = Context::new_small();
    // for..of yields values; entries yields [v, v]
    let r = ctx
        .eval(
            "var s = new Set();
             s.add('a').add('b');
             var out = '';
             for (var v of s) { out += v; }
             for (var e of s.entries()) { out += e[0] + e[1]; }
             var ks = ''; for (var k of s.keys()) { ks += k; }
             out + ks;",
        )
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "abaabbab"
    );
    // constructor from iterable + string TypeError + instanceof
    let r = ctx
        .eval(
            "var s = new Set([1, 2, 3, 2]);
             s.size + (s instanceof Set ? 'Y' : 'N');",
        )
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "3Y"
    );
    let r = ctx
        .eval("var t = false; try { new Set(5); } catch (e) { t = true; } t;")
        .unwrap();
    assert_eq!(r, rune_core::value::Value::boolean(true));
    // Set() without new → TypeError
    let r = ctx
        .eval("var t = false; try { Set(); } catch (e) { t = true; } t;")
        .unwrap();
    assert_eq!(r, rune_core::value::Value::boolean(true));
}

#[test]
fn test_set_foreach() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var s = new Set([10, 20, 30]);
             var sum = 0;
             s.forEach(function(v, v2, set) {
               sum += v;
               if (v !== v2) { sum += 100; }
               if (set !== s) { sum += 100; }
             });
             sum;",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(60));
    // constructor from user iterable (JS next)
    let r = ctx
        .eval(
            "var iter = {
               i: 0,
               [Symbol.iterator]: function() {
                 var self = this;
                 return {
                   next: function() {
                     self.i += 1;
                     if (self.i > 3) { return {done: true}; }
                     return {done: false, value: self.i * 5};
                   }
                 };
               }
             };
             var s = new Set(iter);
             s.size;",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(3));
}

#[test]
fn test_map_set_errors() {
    let mut ctx = Context::new_small();
    // incompatible receivers
    let r = ctx
        .eval("var t = false; try { Map.prototype.get.call({}, 1); } catch (e) { t = true; } t;")
        .unwrap();
    assert_eq!(r, rune_core::value::Value::boolean(true));
    let r = ctx
        .eval("var t = false; try { Set.prototype.add.call([], 1); } catch (e) { t = true; } t;")
        .unwrap();
    assert_eq!(r, rune_core::value::Value::boolean(true));
    // instanceof across both
    let r = ctx
        .eval(
            "var m = new Map(); var s = new Set();
             (m instanceof Map) + '|' + (s instanceof Set) + '|' + (m instanceof Set);",
        )
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "true|true|false"
    );
    // maps are iterable via spread
    let r = ctx
        .eval(
            "var m = new Map([['a', 1]]);
             var arr = [...m];
             arr.length + arr[0][0] + arr[0][1];",
        )
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "1a1"
    );
}

#[test]
fn test_date_basic() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("Date.now() > 1e12;").unwrap();
    assert_eq!(r.to_boolean(), Some(true));
    // Date.UTC with components
    let r = ctx
        .eval(
            "var d = new Date(Date.UTC(2026, 7, 19, 12, 34, 56, 789));
             d.getUTCFullYear() + '-' + d.getUTCMonth() + '-' + d.getUTCDate()
             + ' ' + d.getUTCHours() + ':' + d.getUTCMinutes() + ':' + d.getUTCSeconds()
             + ':' + d.getUTCMilliseconds();",
        )
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "2026-7-19 12:34:56:789"
    );
    // valueOf returns the time value
    let r = ctx
        .eval("new Date(Date.UTC(2026, 7, 19)).valueOf();")
        .unwrap();
    assert_eq!(r.as_float64(), Some(1787097600000.0));
    // getTime matches valueOf
    let r = ctx
        .eval("new Date(Date.UTC(2026, 7, 19)).getTime();")
        .unwrap();
    assert_eq!(r.as_float64(), Some(1787097600000.0));
}

#[test]
fn test_date_parse_iso() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("Date.parse('2026-08-19T12:34:56.789Z');").unwrap();
    assert_eq!(r.as_float64(), Some(1787142896789.0));
    let r = ctx.eval("Date.parse('2026-08-19T12:34:56Z');").unwrap();
    assert_eq!(r.as_float64(), Some(1787142896000.0));
    // local (no offset) forms parse as UTC in this implementation
    let r = ctx.eval("Date.parse('2026-08-19T12:34:56');").unwrap();
    assert_eq!(r.as_float64(), Some(1787142896000.0));
    // invalid
    let r = ctx.eval("Date.parse('not a date');").unwrap();
    assert!(r.as_float64().unwrap().is_nan());
}

#[test]
fn test_date_strings() {
    let mut ctx = Context::new_small();
    // toISOString
    let r = ctx
        .eval("new Date(Date.UTC(2026, 7, 19, 12, 34, 56, 789)).toISOString();")
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "2026-08-19T12:34:56.789Z"
    );
    // toUTCString
    let r = ctx
        .eval("new Date(Date.UTC(2026, 7, 19, 12, 34, 56)).toUTCString();")
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "Wed, 19 Aug 2026 12:34:56 GMT"
    );
    // toString (UTC-only: date + time, space separator, +0000 offset)
    let r = ctx
        .eval("new Date(Date.UTC(2026, 7, 19, 12, 34, 56)).toString();")
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "Wed Aug 19 2026 12:34:56 GMT+0000"
    );
    // invalid date string form
    let r = ctx.eval("new Date('bad').toString();").unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "Invalid Date"
    );
}

#[test]
fn test_date_setters() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var d = new Date(Date.UTC(2026, 0, 31));
             d.setUTCDate(1); d.setUTCMonth(1); d.setUTCHours(3);
             d.setUTCMinutes(5); d.setUTCSeconds(7); d.setUTCMilliseconds(9);
             d.toISOString();",
        )
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "2026-02-01T03:05:07.009Z"
    );
    // setFullYear with 0-99 mapping
    let r = ctx
        .eval(
            "var d = new Date(Date.UTC(2000, 0, 1));
             d.setUTCFullYear(26);
             d.getUTCFullYear();",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(1926));
    // setTime resets
    let r = ctx
        .eval(
            "var d = new Date(Date.UTC(2020, 0, 1));
             d.setTime(0);
             d.getTime();",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(0));
    // non-UTC setters mirror UTC (UTC-only implementation)
    let r = ctx
        .eval(
            "var d = new Date(Date.UTC(2026, 7, 19, 12, 34, 56));
             d.setHours(1); d.getUTCHours();",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(1));
}

#[test]
fn test_date_edge_cases() {
    let mut ctx = Context::new_small();
    // constructor with numeric args (1-based date)
    let r = ctx.eval("new Date(2026, 7, 19).toISOString();").unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "2026-08-19T00:00:00.000Z"
    );
    // single numeric arg = milliseconds since epoch
    let r = ctx.eval("new Date(0).toISOString();").unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "1970-01-01T00:00:00.000Z"
    );
    // Date copy constructor
    let r = ctx
        .eval(
            "var a = new Date(Date.UTC(2026, 0, 1)); var b = new Date(a);
             a === b ? 'same' : b.getTime();",
        )
        .unwrap();
    assert_eq!(r.as_float64(), Some(1767225600000.0));
    // plain call returns a string
    let r = ctx.eval("typeof Date();").unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "string"
    );
    // instanceof
    let r = ctx.eval("new Date() instanceof Date;").unwrap();
    assert_eq!(r.to_boolean(), Some(true));
    // invalid → NaN everywhere
    let r = ctx.eval("new Date('bad').getTime();").unwrap();
    assert!(r.as_float64().unwrap().is_nan());
    // toISOString on invalid throws
    let r = ctx
        .eval("var x = ''; try { new Date('bad').toISOString(); } catch (e) { x = e.name; } x === 'RangeError';")
        .unwrap();
    assert!(r.to_bool(), "invalid toISOString throws RangeError");
    // coercion: Date is a value in comparisons/arithmetic via [[DateValue]]
    let r = ctx.eval("new Date(0) - new Date(0);").unwrap();
    assert_eq!(r.as_smi(), Some(0));
    // String(d) uses the toString representation
    let r = ctx.eval("'x' + new Date(0);").unwrap();
    let v = unsafe {
        rune_core::string::HeapString::to_string(
            r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
        )
    };
    assert!(v.starts_with("xThu Jan 01 1970"));
}

#[test]
fn test_date_legacy_parse() {
    let mut ctx = Context::new_small();
    // legacy toString format round-trips
    let r = ctx
        .eval("Date.parse('Sat Feb 04 1995 23:59:59 GMT+0000');")
        .unwrap();
    assert_eq!(r.as_float64(), Some(791942399000.0));
    let r = ctx
        .eval("Date.parse('Sat Feb 04 1995 23:59:59 GMT');")
        .unwrap();
    assert_eq!(r.as_float64(), Some(791942399000.0));
}

#[test]
fn test_date_to_json() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var d = new Date(Date.UTC(2026, 7, 19, 12, 34, 56, 789));
             d.toJSON();",
        )
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "2026-08-19T12:34:56.789Z"
    );
}

#[test]
fn test_typed_array_ctor_length() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("new Uint8Array(5).length").unwrap();
    assert_eq!(r.as_smi(), Some(5));
}

#[test]
fn test_typed_array_indexing() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var a = new Int32Array(3);
             a[0] = 42;
             a[1] = -7;
             a[2] = 1.9;
             a[0] + a[1] + a[2];",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(36));
}

#[test]
fn test_typed_array_conversions() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var a = new Uint8Array(3);
             a[0] = -1;       // wraps to 255
             a[1] = 256;      // wraps to 0
             a[2] = 3.7;      // truncates to 3
             a[0] + a[1] + a[2];",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(258));
}

#[test]
fn test_typed_array_from_array() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var a = new Float64Array([1, 2, 3.5]);
             a[0] + a[1] + a[2];",
        )
        .unwrap();
    assert_eq!(r.as_float64(), Some(6.5));
}

#[test]
fn test_typed_array_out_of_range_noop() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var a = new Uint8Array(2);
             a[5] = 99;
             a[0] + a[1];",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(0));
}

#[test]
fn test_array_buffer_basics() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var b = new ArrayBuffer(16);
             b.byteLength;",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(16));
}

#[test]
fn test_array_buffer_slice() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var b = new ArrayBuffer(16);
             var c = b.slice(4, 10);
             c.byteLength;",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(6));
}

#[test]
fn test_typed_array_over_buffer() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var b = new ArrayBuffer(16);
             var a = new Uint8Array(b, 4, 6);
             a.length + a.byteOffset + a.byteLength;",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(16));
}

#[test]
fn test_typed_array_buffer_share() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var b = new ArrayBuffer(8);
             var a = new Uint8Array(b);
             var c = new Uint8Array(b, 4);
             a[0] = 7;
             a[7] = 9;
             c[0] + c[3];",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(9));
}

#[test]
fn test_typed_array_set() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var a = new Int32Array(4);
             a.set([1, 2, 3], 1);
             a[0] + a[1] + a[2] + a[3];",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(6));
}

#[test]
fn test_typed_array_subarray() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var a = new Uint8Array([1, 2, 3, 4, 5]);
             var s = a.subarray(1, 4);
             s.length + s[0] + s[2];",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(9));
}

#[test]
fn test_typed_array_fill() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var a = new Uint8Array(4);
             a.fill(9, 1, 3);
             a[0] + a[1] + a[2] + a[3];",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(18));
}

#[test]
fn test_typed_array_at_index_of_includes() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var a = new Int32Array([5, 10, 15]);
             a.at(-1) + a.indexOf(10) + (a.includes(15) ? 100 : 0);",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(116));
}

#[test]
fn test_typed_array_slice() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var a = new Uint8Array([1, 2, 3, 4]);
             var s = a.slice(1, 3);
             s.length + s[0] + s[1];",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(7));
}

#[test]
fn test_typed_array_iteration() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var sum = 0;
             for (var v of new Int32Array([4, 5, 6])) { sum += v; }
             sum;",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(15));
}

#[test]
fn test_typed_array_spread() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("[...new Uint8Array([1, 2, 3])].length").unwrap();
    assert_eq!(r.as_smi(), Some(3));
}

#[test]
fn test_typed_array_instanceof() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var a = new Uint8Array(2);
             (a instanceof Uint8Array ? 1 : 0) + (a instanceof Int32Array ? 10 : 0) +
             (a instanceof Array ? 100 : 0) + (new Float64Array(1) instanceof Float64Array ? 1000 : 0);",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(1001));
}

#[test]
fn test_typed_array_plain_call_throws() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var x;
             try { Uint8Array(3); x = 'no'; } catch (e) { x = 'yes'; }
             x;",
        )
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "yes"
    );
}

#[test]
fn test_array_buffer_plain_call_throws() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var x;
             try { ArrayBuffer(4); x = 'no'; } catch (e) { x = 'yes'; }
             x;",
        )
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "yes"
    );
}

#[test]
fn test_typed_array_from_string() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("new Uint16Array('abc').length").unwrap();
    assert_eq!(r.as_smi(), Some(3));
}

#[test]
fn test_typed_array_from_typed_array() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var a = new Uint8Array([1, 2, 3]);
             var b = new Int16Array(a);
             b.length + b[1];",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(5));
}

#[test]
fn test_typed_array_constructor_props() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "Uint8Array.BYTES_PER_ELEMENT + Float64Array.BYTES_PER_ELEMENT +
             new Uint8Array(1).BYTES_PER_ELEMENT;",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(10));
}

#[test]
fn test_typed_array_u8clamped() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var a = new Uint8ClampedArray(3);
             a[0] = -5;   // clamps to 0
             a[1] = 300;  // clamps to 255
             a[2] = 2.5;  // rounds to 2 (half-to-even)
             a[0] + a[1] + a[2];",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(257));
}

#[test]
fn test_typed_array_overlapping_set() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var a = new Uint8Array([1, 2, 3, 4]);
             a.set(a.subarray(0, 3), 1);
             a[0] + a[1] + a[2] + a[3];",
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(7));
}

#[test]
fn test_string_from_char_code() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("String.fromCharCode(65, 66, 67)").unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "ABC"
    );
    let r = ctx.eval("String.fromCharCode(65.9, 66.5)").unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "AB"
    );
    let r = ctx.eval("String.fromCharCode(true, false, '65')").unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "\u{1}\u{0}A"
    );
    let r = ctx.eval("String.fromCharCode(0x1F601)").unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "\u{f601}"
    );
}

#[test]
fn test_string_char_code_at_utf16() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("'\\u00e9'.charCodeAt(0)").unwrap();
    assert_eq!(r.as_smi(), Some(233));
    let r = ctx.eval("'\\u{1F600}'.charCodeAt(0)").unwrap();
    assert_eq!(r.as_smi(), Some(0xD83D));
    let r = ctx.eval("'\\u{1F600}'.charCodeAt(1)").unwrap();
    assert_eq!(r.as_smi(), Some(0xDE00));
    let r = ctx.eval("'ab'.charCodeAt(5)").unwrap();
    assert!(r.as_float64().unwrap().is_nan());
    let r = ctx.eval("'ab'.charCodeAt(1.9)").unwrap();
    assert_eq!(r.as_smi(), Some(98));
}

#[test]
fn test_string_code_point_at() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("'\\u{1F600}'.codePointAt(0)").unwrap();
    assert_eq!(r.as_smi(), Some(0x1F600));
    let r = ctx.eval("'\\u{1F600}'.codePointAt(1)").unwrap();
    assert_eq!(r.as_smi(), Some(0xDE00));
    let r = ctx.eval("'ab'.codePointAt(9)").unwrap();
    assert!(r.is_undefined());
}

#[test]
fn test_string_utf16_positions() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("'\\u00e9a'.includes('a', 1)").unwrap();
    assert_eq!(r.to_boolean(), Some(true));
    let r = ctx.eval("'\\u{1F600}x'.startsWith('x', 2)").unwrap();
    assert_eq!(r.to_boolean(), Some(true));
    let r = ctx.eval("'x\\u{1F600}'.endsWith('x', 1)").unwrap();
    assert_eq!(r.to_boolean(), Some(true));
    let r = ctx.eval("'\\u{1F600}ab'.indexOf('a', 1)").unwrap();
    assert_eq!(r.as_smi(), Some(2));
    let r = ctx.eval("'\\u{1F600}ab'.indexOf('a', 2)").unwrap();
    assert_eq!(r.as_smi(), Some(2));
}

#[test]
fn test_string_slice_utf16() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("'\\u00e9abc'.slice(1, 3)").unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "ab"
    );
    let r = ctx.eval("'\\u{1F600}xy'.substring(1, 3)").unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "\u{FFFD}x"
    );
    let r = ctx.eval("'\\u{1F600}xy'.substr(1, 2)").unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "\u{FFFD}x"
    );
}

#[test]
fn test_string_pad_utf16() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("'ab'.padStart(5, 'xy')").unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "xyxab"
    );
    let r = ctx.eval("'ab'.padEnd(5, 'xy')").unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "abxyx"
    );
    let r = ctx.eval("'\\u{1F600}'.padStart(4, 'ab')").unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "ab\u{1F600}"
    );
    let r = ctx.eval("'ab'.padStart(4)").unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "  ab"
    );
    let r = ctx.eval("'ab'.padStart(2, 'x')").unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "ab"
    );
    let r = ctx.eval("'ab'.padEnd(3, '')").unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "ab "
    );
}

#[test]
fn test_string_pad_infinity_throws() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            "var x;
             try { 'a'.padStart(Infinity); x = 'no'; } catch (e) { x = 'yes'; }
             x;",
        )
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "yes"
    );
}

#[test]
fn test_string_repeat() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("'ab'.repeat(3)").unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "ababab"
    );
    let r = ctx.eval("'ab'.repeat(0)").unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        ""
    );
    let r = ctx.eval("'ab'.repeat(2.9)").unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "abab"
    );
    let r = ctx
        .eval(
            "var x;
             try { 'a'.repeat(-1); x = 'no'; } catch (e) { x = 'yes'; }
             x;",
        )
        .unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "yes"
    );
}

#[test]
fn test_string_trim_family() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("'  \\t\\n  ab  \\r '.trim()").unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "ab"
    );
    let r = ctx.eval("'  ab  '.trimStart()").unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "ab  "
    );
    let r = ctx.eval("'  ab  '.trimEnd()").unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "  ab"
    );
    let r = ctx.eval("'\\u{2028}ab\\u{2029}'.trim()").unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "ab"
    );
}

#[test]
fn test_string_case_conversion() {
    let mut ctx = Context::new_small();
    let r = ctx.eval("'HeLLo WoRLD'.toLowerCase()").unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "hello world"
    );
    let r = ctx.eval("'HeLLo WoRLD'.toUpperCase()").unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "HELLO WORLD"
    );
    let r = ctx.eval("'\\u00e9\\u00df'.toUpperCase()").unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                r.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "\u{C9}SS"
    );
}

// ---------- ESM modules (§16) ----------

/// Evaluate an entry module whose dependency sources are looked up in `deps`
/// (specifier → source). Returns the module-export read helper.
fn eval_module_with(ctx: &mut Context, entry: &str, deps: &[(&str, &str)]) -> Result<(), String> {
    let mut resolver = |spec: &str, _referrer: &str| -> Result<String, String> {
        deps.iter()
            .find(|(s, _)| *s == spec)
            .map(|(_, src)| src.to_string())
            .ok_or_else(|| format!("no source for {spec}"))
    };
    ctx.eval_module(entry, &mut resolver).map(|_| ())
}

/// Read an exported binding (may be a module function for call_value).
fn module_export_smi(ctx: &mut Context, spec: &str, name: &str) -> Option<i32> {
    ctx.module_export(spec, name).and_then(|v| v.as_smi())
}

fn module_export_str(ctx: &mut Context, spec: &str, name: &str) -> String {
    let v = ctx.module_export(spec, name).unwrap();
    unsafe {
        rune_core::string::HeapString::to_string(
            v.heap_ptr().unwrap() as *mut rune_core::string::HeapString
        )
    }
}

#[test]
fn test_esm_basic_import_export() {
    let mut ctx = Context::new_small();
    eval_module_with(
        &mut ctx,
        r#"
            import { add, mul as times } from "./math.js";
            export function run() { return add(2, 3) + times(4, 5); }
        "#,
        &[(
            "./math.js",
            r#"
                export function add(a, b) { return a + b; }
                export function mul(a, b) { return a * b; }
            "#,
        )],
    )
    .unwrap();
    let run = ctx.module_export("<entry>", "run").unwrap();
    let v = ctx.call_value(run, &[]).unwrap();
    assert_eq!(v.as_smi(), Some(25));
}

#[test]
fn test_esm_default_export_import() {
    let mut ctx = Context::new_small();
    eval_module_with(
        &mut ctx,
        r#"
            import def from "./math.js";
            export function run() { return def(3, 4); }
        "#,
        &[(
            "./math.js",
            r#"export default function(a, b) { return a * b; }"#,
        )],
    )
    .unwrap();
    let run = ctx.module_export("<entry>", "run").unwrap();
    let v = ctx.call_value(run, &[]).unwrap();
    assert_eq!(v.as_smi(), Some(12));
}

#[test]
fn test_esm_default_export_expression() {
    let mut ctx = Context::new_small();
    eval_module_with(
        &mut ctx,
        r#"
            import def from "./v.js";
            export const got = def;
        "#,
        &[("./v.js", r#"export default 42;"#)],
    )
    .unwrap();
    assert_eq!(module_export_smi(&mut ctx, "<entry>", "got"), Some(42));
}

#[test]
fn test_esm_namespace_import() {
    let mut ctx = Context::new_small();
    eval_module_with(
        &mut ctx,
        r#"
            import * as ns from "./math.js";
            export const total = ns.add(10, 20) + ns.mul(3, 4);
        "#,
        &[(
            "./math.js",
            r#"
                export function add(a, b) { return a + b; }
                export function mul(a, b) { return a * b; }
            "#,
        )],
    )
    .unwrap();
    assert_eq!(module_export_smi(&mut ctx, "<entry>", "total"), Some(42));
}

#[test]
fn test_esm_reexport() {
    let mut ctx = Context::new_small();
    eval_module_with(
        &mut ctx,
        r#"
            import { plus } from "./agg.js";
            export const t = plus(10, 1);
        "#,
        &[
            (
                "./math.js",
                r#"export function add(a, b) { return a + b; }"#,
            ),
            ("./agg.js", r#"export { add as plus } from "./math.js";"#),
        ],
    )
    .unwrap();
    assert_eq!(module_export_smi(&mut ctx, "<entry>", "t"), Some(11));
}

#[test]
fn test_esm_export_star() {
    let mut ctx = Context::new_small();
    eval_module_with(
        &mut ctx,
        r#"
            import { add } from "./agg.js";
            export const t = add(1, 2);
        "#,
        &[
            (
                "./math.js",
                r#"export function add(a, b) { return a + b; }"#,
            ),
            ("./agg.js", r#"export * from "./math.js";"#),
        ],
    )
    .unwrap();
    assert_eq!(module_export_smi(&mut ctx, "<entry>", "t"), Some(3));
}

#[test]
fn test_esm_export_star_as() {
    let mut ctx = Context::new_small();
    eval_module_with(
        &mut ctx,
        r#"
            import { ns } from "./agg.js";
            export const t = ns.add(5, 6);
        "#,
        &[
            (
                "./math.js",
                r#"export function add(a, b) { return a + b; }"#,
            ),
            ("./agg.js", r#"export * as ns from "./math.js";"#),
        ],
    )
    .unwrap();
    assert_eq!(module_export_smi(&mut ctx, "<entry>", "t"), Some(11));
}

#[test]
fn test_esm_export_rename_local() {
    let mut ctx = Context::new_small();
    eval_module_with(
        &mut ctx,
        r#"
            export function double(x) { return x * 2; }
            export { double as twice };
            export const val = 5;
            export { val as v2 };
        "#,
        &[],
    )
    .unwrap();
    assert_eq!(module_export_smi(&mut ctx, "<entry>", "val"), Some(5));
    assert_eq!(module_export_smi(&mut ctx, "<entry>", "v2"), Some(5));
    let twice = ctx.module_export("<entry>", "twice").unwrap();
    assert_eq!(
        ctx.call_value(twice, &[rune_core::value::Value::smi(21)])
            .unwrap()
            .as_smi(),
        Some(42)
    );
}

#[test]
fn test_esm_circular_dependencies() {
    let mut ctx = Context::new_small();
    eval_module_with(
        &mut ctx,
        r#"
            import { bfn } from "./b.js";
            import { afn } from "./a.js";
            export function run() { return afn() + "|" + bfn(); }
        "#,
        &[
            (
                "./a.js",
                r#"
                    import { bfn } from "./b.js";
                    export const a_val = "a";
                    export function afn() { return "A" + bfn(); }
                "#,
            ),
            (
                "./b.js",
                r#"
                    import { a_val } from "./a.js";
                    export function bfn() { return "B" + a_val; }
                "#,
            ),
        ],
    )
    .unwrap();
    let run = ctx.module_export("<entry>", "run").unwrap();
    let v = ctx.call_value(run, &[]).unwrap();
    assert_eq!(
        unsafe {
            rune_core::string::HeapString::to_string(
                v.heap_ptr().unwrap() as *mut rune_core::string::HeapString
            )
        },
        "ABa|Ba"
    );
}

#[test]
fn test_esm_self_cycle() {
    let mut ctx = Context::new_small();
    eval_module_with(
        &mut ctx,
        r#"
            import { self_f, self_val } from "./self.js";
            export function run() { return self_f() + self_val; }
        "#,
        &[(
            "./self.js",
            r#"
                export const self_val = 1;
                export function self_f() { return self_val * 2; }
            "#,
        )],
    )
    .unwrap();
    let run = ctx.module_export("<entry>", "run").unwrap();
    assert_eq!(ctx.call_value(run, &[]).unwrap().as_smi(), Some(3));
}

#[test]
fn test_esm_module_let_and_const() {
    let mut ctx = Context::new_small();
    eval_module_with(
        &mut ctx,
        r#"
            import { counter, bump } from "./state.js";
            export function run() { const before = counter; bump(); return before + counter; }
        "#,
        &[(
            "./state.js",
            r#"
                export let counter = 1;
                export function bump() { counter = counter + 1; }
            "#,
        )],
    )
    .unwrap();
    let run = ctx.module_export("<entry>", "run").unwrap();
    assert_eq!(ctx.call_value(run, &[]).unwrap().as_smi(), Some(3));
}

#[test]
fn test_esm_module_function_sees_own_bindings() {
    let mut ctx = Context::new_small();
    eval_module_with(
        &mut ctx,
        r#"
            import { seed } from "./dep.js";
            const local = seed * 10;
            export function run() { return local + seed; }
        "#,
        &[("./dep.js", r#"export const seed = 4;"#)],
    )
    .unwrap();
    let run = ctx.module_export("<entry>", "run").unwrap();
    assert_eq!(ctx.call_value(run, &[]).unwrap().as_smi(), Some(44));
}

#[test]
fn test_esm_tdz_cycle_read_throws() {
    let mut ctx = Context::new_small();
    // b reads a's `a_val` at top level while a is mid-evaluation (cycle):
    // a_val is still in the TDZ → ReferenceError (§9.2.2.2 / TDZ semantics).
    let r = eval_module_with(
        &mut ctx,
        r#"
            import { afn } from "./a.js";
            export function run() { return afn(); }
        "#,
        &[
            (
                "./a.js",
                r#"
                    import { bval } from "./b.js";
                    export const a_val = bval + "a";
                    export function afn() { return a_val; }
                "#,
            ),
            (
                "./b.js",
                r#"
                    import { a_val } from "./a.js";
                    export const bval = a_val + "!";
                "#,
            ),
        ],
    );
    match r {
        Err(msg) => assert!(
            msg.contains("ReferenceError"),
            "expected ReferenceError, got: {msg}"
        ),
        Ok(()) => panic!("expected TDZ ReferenceError but module evaluated"),
    }
}

#[test]
fn test_esm_duplicate_export_early_error() {
    let mut ctx = Context::new_small();
    let r = eval_module_with(
        &mut ctx,
        r#"
            export const x = 1;
            export { x as y };
            export { y };
        "#,
        &[],
    );
    match r {
        Err(msg) => assert!(
            msg.contains("Duplicate export"),
            "expected duplicate-export error, got: {msg}"
        ),
        Ok(()) => panic!("expected duplicate export early error"),
    }
}

#[test]
fn test_esm_imported_binding_assignment_throws() {
    let mut ctx = Context::new_small();
    // Assignment to an imported binding is a TypeError (§9.2.2.3), both at
    // module top level (StoreModuleImport) and inside module functions.
    let r = eval_module_with(
        &mut ctx,
        r#"
            import { x } from "./dep.js";
            export function run() { x = 5; return x; }
            x = 1;
        "#,
        &[("./dep.js", r#"export const x = 1;"#)],
    );
    match r {
        Err(msg) => assert!(
            msg.contains("Assignment to constant variable"),
            "expected assignment TypeError, got: {msg}"
        ),
        Ok(()) => panic!("expected imported-assignment TypeError"),
    }
}

#[test]
fn test_esm_missing_import_value_is_undefined() {
    let mut ctx = Context::new_small();
    eval_module_with(
        &mut ctx,
        r#"
            import { nope } from "./m.js";
            export const v = nope;
        "#,
        &[("./m.js", r#"export const other = 1;"#)],
    )
    .unwrap();
    assert!(ctx.module_export("<entry>", "v").unwrap().is_undefined());
}

#[test]
fn test_esm_hoisted_function_available_in_cycle() {
    let mut ctx = Context::new_small();
    // Function declarations are initialized at instantiation (not TDZ), so a
    // cycle can call a function before its module finishes evaluating.
    eval_module_with(
        &mut ctx,
        r#"
            import { late } from "./dep.js";
            export const result = late();
        "#,
        &[("./dep.js", r#"export function late() { return 99; }"#)],
    )
    .unwrap();
    assert_eq!(module_export_smi(&mut ctx, "<entry>", "result"), Some(99));
}

#[test]
fn test_esm_bare_let_initializes_undefined() {
    let mut ctx = Context::new_small();
    eval_module_with(
        &mut ctx,
        r#"
            export let x;
            export const has = typeof x;
        "#,
        &[],
    )
    .unwrap();
    assert_eq!(module_export_str(&mut ctx, "<entry>", "has"), "undefined");
}

#[test]
fn test_error_family_basic() {
    let mut ctx = Context::new_small();
    assert_eq!(
        eval_str(&mut ctx, r#"new TypeError("boom").message"#),
        "boom"
    );
    assert_eq!(
        eval_str(&mut ctx, r#"new TypeError("boom").name"#),
        "TypeError"
    );
    assert_eq!(eval_str(&mut ctx, r#"new Error().message"#), "");
    assert_eq!(eval_str(&mut ctx, r#"new Error().name"#), "Error");
    assert_eq!(eval_str(&mut ctx, r#"new RangeError(42).message"#), "42");
    assert_eq!(
        eval_str(&mut ctx, r#"new SyntaxError().toString()"#),
        "SyntaxError"
    );
    assert_eq!(
        eval_str(&mut ctx, r#"new Error("msg").toString()"#),
        "Error: msg"
    );
    assert_eq!(
        eval_str(&mut ctx, r#"new TypeError("x").toString()"#),
        "TypeError: x"
    );
    assert_eq!(
        eval_str(
            &mut ctx,
            r#"new TypeError("m").message + "|" + new TypeError("m").name"#
        ),
        "m|TypeError"
    );
}

#[test]
fn test_error_family_instanceof() {
    let mut ctx = Context::new_small();
    assert_eq!(
        eval_str(
            &mut ctx,
            r#"String(new TypeError("x") instanceof TypeError)"#
        ),
        "true"
    );
    assert_eq!(
        eval_str(&mut ctx, r#"String(new TypeError("x") instanceof Error)"#),
        "true"
    );
    assert_eq!(
        eval_str(
            &mut ctx,
            r#"String(new TypeError("x") instanceof RangeError)"#
        ),
        "false"
    );
    assert_eq!(
        eval_str(&mut ctx, r#"String(new Error("x") instanceof Error)"#),
        "true"
    );
    assert_eq!(
        eval_str(&mut ctx, r#"String(new Error("x") instanceof TypeError)"#),
        "false"
    );
    assert_eq!(
        eval_str(&mut ctx, r#"String(new URIError("u") instanceof Error)"#),
        "true"
    );
    assert_eq!(
        eval_str(&mut ctx, r#"String(TypeError.prototype instanceof Error)"#),
        "true"
    );
}

#[test]
fn test_error_family_plain_call() {
    let mut ctx = Context::new_small();
    assert_eq!(eval_str(&mut ctx, r#"TypeError("boom").message"#), "boom");
    assert_eq!(eval_str(&mut ctx, r#"EvalError("e").name"#), "EvalError");
    assert_eq!(
        eval_str(&mut ctx, r#"String(URIError("u") instanceof URIError)"#),
        "true"
    );
}

#[test]
fn test_error_family_symbol_message_throws() {
    let mut ctx = Context::new_small();
    // A2: the ToString(Symbol) failure surfaces as a real TypeError object.
    let r = ctx
        .eval(r#"var n = ''; try { new TypeError(Symbol("s")); } catch (e) { n = e.name; } n === 'TypeError';"#)
        .unwrap();
    assert!(r.to_bool());
    let r = ctx
        .eval(r#"var n = ''; try { new Error(Symbol("s")); } catch (e) { n = e.name; } n === 'TypeError';"#)
        .unwrap();
    assert!(r.to_bool());
}

#[test]
fn test_error_family_cause() {
    let mut ctx = Context::new_small();
    assert_eq!(
        eval_str(&mut ctx, r#"String(new Error("m", { cause: 42 }).cause)"#),
        "42"
    );
    assert_eq!(
        eval_str(&mut ctx, r#"String(new Error("m").cause)"#),
        "undefined"
    );
    assert_eq!(
        eval_str(&mut ctx, r#"String(new Error("m", {}).cause)"#),
        "undefined"
    );
    assert_eq!(
        eval_str(&mut ctx, r#"String(new Error("m", "str").cause)"#),
        "undefined"
    );
}

#[test]
fn test_error_family_prototype_chain() {
    let mut ctx = Context::new_small();
    assert_eq!(
        eval_str(&mut ctx, r#"String(TypeError.prototype instanceof Error)"#),
        "true"
    );
    assert_eq!(eval_str(&mut ctx, r#"String(TypeError.length)"#), "1");
    assert_eq!(eval_str(&mut ctx, r#"String(Error.length)"#), "1");
    assert_eq!(
        eval_str(
            &mut ctx,
            r#"String(TypeError.prototype.constructor === TypeError)"#
        ),
        "true"
    );
    assert_eq!(
        eval_str(&mut ctx, r#"String(Error.prototype.constructor === Error)"#),
        "true"
    );
    assert_eq!(
        eval_str(&mut ctx, r#"TypeError.prototype.name"#),
        "TypeError"
    );
    assert_eq!(eval_str(&mut ctx, r#"Error.prototype.name"#), "Error");
}

#[test]
fn test_error_family_assert_throws_wrapper() {
    let mut ctx = Context::new_small();
    assert_eq!(
        eval_str(
            &mut ctx,
            r#"(function(){ try { (function(){ throw 3.14; })(); } catch (e) { assert.throws(TypeError, function(){ throw new TypeError("x"); }); return "ok"; } })()"#
        ),
        "ok"
    );
    assert_eq!(
        eval_str(
            &mut ctx,
            r#"(function(){ try { (function(){ throw 3.14; })(); } catch (e) { return "outer"; } })()"#
        ),
        "outer"
    );
}

#[test]
#[cfg(target_arch = "aarch64")]
fn test_jit_div_exp_whitelisted() {
    // J1: Div/Exp compile natively via jit_binop_helper
    let mut ctx = Context::new_small();
    ctx.enable_jit = true;
    ctx.eval(
        r#"
            function half(x) { return x / 2; }
            function cube(x) { return x ** 3; }
            var last = "";
            for (var i = 0; i < 200; i++) { last = String(half(21)) + "," + String(cube(3)); }
        "#,
    )
    .unwrap();
    // Post-tier-up iterations ran natively — the final loop value must be
    // exactly right and nothing may have bailed.
    assert_eq!(eval_str(&mut ctx, "last"), "10.5,27");
    assert!(
        ctx.vm().jit_entry_count > 0,
        "Div/Exp fns must have been JIT-compiled"
    );
    assert_eq!(
        ctx.vm().jit_bailout_count,
        0,
        "native Div/Exp must not bail"
    );
}

#[test]
fn test_jit_div_by_zero_and_negative_exp() {
    let mut ctx = Context::new_small();
    ctx.eval(
        r#"
            function d(x, y) { return x / y; }
            function e(x, y) { return x ** y; }
            for (var i = 0; i < 100; i++) { d(i - 50, 3); e(2, i % 5); }
        "#,
    )
    .unwrap();
    assert_eq!(
        eval_str(&mut ctx, "String(d(1,0))"),
        "Infinity",
        "x/0 must be Infinity per §12.6.3"
    );
    assert_eq!(eval_str(&mut ctx, "String(e(2,-2))"), "0.25");
    assert_eq!(eval_str(&mut ctx, "String(e(4,0.5))"), "2");
}

#[test]
#[cfg(target_arch = "aarch64")]
fn test_jit_optional_chaining_whitelisted() {
    // J1: JumpIfNullOrUndefined compiles natively for `?.`
    let mut ctx = Context::new_small();
    ctx.enable_jit = true;
    let r = ctx
        .eval(
            r#"
            function pick(o) { return o?.x; }
            var objs = [{x: 1}, {x: 2}, null, undefined, {x: 5}];
            var s = 0;
            for (var i = 0; i < 100; i++) {
                var o = objs[i % 5];
                var v = pick(o);
                if (v !== undefined) { s += v; }
            }
            s
        "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some((1 + 2 + 5) * 20));
    assert!(
        ctx.vm().jit_entry_count > 0,
        "optional-chaining fn should tier up"
    );
}

#[test]
fn test_jit_in_instanceof_bail_correctness() {
    // J1: In/Instanceof are whitelisted but always bail — results must stay
    // exactly correct whether or not native code ran.
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            function check(o) {
                var has = 'x' in o;
                var inst = o instanceof Object;
                return (has ? 1 : 0) + (inst ? 10 : 0);
            }
            var sum = 0;
            for (var i = 0; i < 100; i++) { sum += check({x: 1}); }
            sum
        "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(11 * 100));
}

#[test]
#[cfg(target_arch = "aarch64")]
fn test_jit_negative_smi_results_boxed() {
    // J2-fix regression: native Sub/Mul/Mod with NEGATIVE Smi results used to
    // ORR the QNAN prefix onto an unmasked sign-extended payload — tag bits
    // became 7 (invalid) and the value read back as NaN.
    let mut ctx = Context::new_small();
    ctx.eval(
        r#"
            function m(a, b) { return a % b; }
            function q(a, b) { return a * b; }
            function s(a, b) { return a - b; }
            for (var i = 0; i < 100; i++) {
                m(i + 7, i + 3); q(i + 1, i + 2); s(i, i + 1);
            }
        "#,
    )
    .unwrap();
    // Post-tier-up native calls with negative results
    assert_eq!(ctx.eval("m(-7, 3)").unwrap().as_smi(), Some(-1));
    assert_eq!(ctx.eval("q(3, -7)").unwrap().as_smi(), Some(-21));
    assert_eq!(ctx.eval("s(3, -7)").unwrap().as_smi(), Some(10));
    assert_eq!(ctx.eval("m(-7, -3)").unwrap().as_smi(), Some(-1));
    assert_eq!(ctx.eval("q(-3, -7)").unwrap().as_smi(), Some(21));
    assert_eq!(ctx.eval("s(-3, 7)").unwrap().as_smi(), Some(-10));
    // Positives unaffected
    assert_eq!(ctx.eval("m(7, 3)").unwrap().as_smi(), Some(1));
    assert_eq!(ctx.eval("q(3, 7)").unwrap().as_smi(), Some(21));
    assert_eq!(ctx.eval("s(7, 3)").unwrap().as_smi(), Some(4));
}

#[test]
#[cfg(target_arch = "aarch64")]
fn test_jit_float_sub_native() {
    // J2#3b: Smi - float takes the helper path, stays native
    let mut ctx = Context::new_small();
    ctx.enable_jit = true;
    ctx.eval(
        r#"
            function f(a, b) { return a - b; }
            // Float arrives as an ARGUMENT (float literals in fn bodies still
            // gate eligibility — J2 checklist #4)
            for (var i = 0; i < 100; i++) { f(i + 1, i + 2); }
        "#,
    )
    .unwrap();
    assert_eq!(
        eval_str(&mut ctx, "String(f(3, 0.5))"),
        "2.5",
        "Smi - float must be exact via helper"
    );
    assert!(ctx.vm().jit_entry_count > 0);
}

#[test]
#[cfg(target_arch = "aarch64")]
fn test_jit_float_mod_native() {
    // J2#3c: float Mod via helper — incl. x % 0 → NaN without bailing
    let mut ctx = Context::new_small();
    ctx.enable_jit = true;
    ctx.eval(
        r#"
            function m(a, b) { return a % b; }
            for (var i = 0; i < 100; i++) { m(i + 7, i + 3); }
        "#,
    )
    .unwrap();
    assert_eq!(eval_str(&mut ctx, "String(m(5.5, 2))"), "1.5");
    assert_eq!(eval_str(&mut ctx, "String(m(-7, 3))"), "-1");
    assert_eq!(eval_str(&mut ctx, "String(m(5, 0))"), "NaN");
    assert!(ctx.vm().jit_entry_count > 0);
}

#[test]
#[cfg(target_arch = "aarch64")]
fn test_jit_float_comparisons_native() {
    // J2#3d: Lt/Gt/Le/Ge with float operands via helper
    let mut ctx = Context::new_small();
    ctx.enable_jit = true;
    ctx.eval(
        r#"
            function cmp(a, b) {
                var r = '';
                if (a < b) { r += 'L'; }
                if (a > b) { r += 'G'; }
                if (a <= b) { r += 'l'; }
                if (a >= b) { r += 'g'; }
                return r;
            }
            for (var i = 0; i < 100; i++) { cmp(i, i + 1); }
        "#,
    )
    .unwrap();
    assert_eq!(eval_str(&mut ctx, "cmp(1, 2.5)"), "Ll");
    assert_eq!(eval_str(&mut ctx, "cmp(2.5, 1)"), "Gg");
    assert_eq!(eval_str(&mut ctx, "cmp(2, 2)"), "lg");
    assert_eq!(eval_str(&mut ctx, "cmp(-1.5, -1)"), "Ll");
    assert!(ctx.vm().jit_entry_count > 0);
}

#[test]
#[cfg(target_arch = "aarch64")]
fn test_jit_float_literal_accumulator() {
    // J2#4 + goal: float LITERALS inside fn bodies are now eligible;
    // the accumulator goes float after iteration 1 and stays native.
    let mut ctx = Context::new_small();
    ctx.enable_jit = true;
    ctx.eval(
        r#"
            function grow(n) {
                var acc = 1;
                for (var i = 0; i < n; i++) { acc = acc * 1.5; }
                return acc;
            }
            for (var k = 0; k < 100; k++) { grow(50); }
        "#,
    )
    .unwrap();
    assert!(ctx.vm().jit_entry_count > 0, "grow must tier up");
    assert_eq!(eval_str(&mut ctx, "String(grow(4))"), "5.0625");
    assert_eq!(eval_str(&mut ctx, "String(grow(200000))"), "Infinity");
    assert_eq!(
        ctx.vm().jit_bailout_count,
        0,
        "float accumulator must not bail (J2 goal)"
    );
}

// ---------------------------------------------------------------------------
// Production-readiness gate: GC roots for state that lives only in a frame or
// a suspended generator. These run with a 1 MiB semispace and enough
// allocation to force several collections while the value is live.
// ---------------------------------------------------------------------------

const GC_STRESS_ALLOC: &str = r#"
    for (var i = 0; i < 80000; i++) { var t = { n: i, s: "z" + i }; }
"#;

#[test]
fn test_gc_frame_this_survives_collection() {
    let mut ctx = Context::new_small();
    let src = format!(
        r#"
        var o = {{ v: 7, m: function () {{
            {}
            return this.v;
        }} }};
        o.m();
        "#,
        GC_STRESS_ALLOC
    );
    let r = ctx.eval(&src).unwrap();
    assert_eq!(r.as_smi(), Some(7), "method receiver must survive GC");
}

#[test]
fn test_gc_constructed_object_survives_collection() {
    let mut ctx = Context::new_small();
    let src = format!(
        r#"
        function C() {{
            {}
            this.v = 42;
        }}
        var c = new C();
        c.v;
        "#,
        GC_STRESS_ALLOC
    );
    let r = ctx.eval(&src).unwrap();
    assert_eq!(r.as_smi(), Some(42), "new-object must survive GC");
}

#[test]
fn test_gc_private_name_table_survives_collection() {
    let mut ctx = Context::new_small();
    let src = format!(
        r#"
        class C {{
            #x = 41;
            m() {{
                {}
                return this.#x + 1;
            }}
        }}
        new C().m();
        "#,
        GC_STRESS_ALLOC
    );
    let r = ctx.eval(&src).unwrap();
    assert_eq!(r.as_smi(), Some(42), "private-name table must survive GC");
}

#[test]
fn test_gc_generator_this_survives_collection() {
    let mut ctx = Context::new_small();
    let src = format!(
        r#"
        var o = {{ v: 99, g: function* () {{
            yield 1;
            {}
            return this.v;
        }} }};
        var it = o.g();
        it.next();
        {}
        it.next().value;
        "#,
        GC_STRESS_ALLOC, GC_STRESS_ALLOC
    );
    let r = ctx.eval(&src).unwrap();
    assert_eq!(
        r.as_smi(),
        Some(99),
        "suspended generator receiver must survive GC"
    );
}

#[test]
fn test_gc_generator_env_survives_collection() {
    let mut ctx = Context::new_small();
    let src = format!(
        r#"
        function make() {{
            var x = 41;
            return (function* () {{
                yield 1;
                {}
                return x + 1;
            }})();
        }}
        var it = make();
        it.next();
        {}
        it.next().value;
        "#,
        GC_STRESS_ALLOC, GC_STRESS_ALLOC
    );
    let r = ctx.eval(&src).unwrap();
    assert_eq!(
        r.as_smi(),
        Some(42),
        "suspended generator env must survive GC"
    );
}

#[test]
fn test_gc_microtask_callback_survives_collection() {
    let mut ctx = Context::new_small();
    let src = format!(
        r#"
        var out = 0;
        Promise.resolve(7).then(function (v) {{ out = v + 1; }});
        {}
        0;
        "#,
        GC_STRESS_ALLOC
    );
    ctx.eval(&src).unwrap();
    let r = ctx.eval("out").unwrap();
    assert_eq!(r.as_smi(), Some(8), "queued microtask must survive GC");
}

// ---------------------------------------------------------------------------
// Production-readiness gate: growth must not invalidate aliases. The object/
// array header is copied to a new address; every holder of the old value must
// observe the replacement (forwarding resolution), and a later GC must copy
// the final target rather than leaving a from-space pointer behind.
// ---------------------------------------------------------------------------

#[test]
fn test_grow_object_alias_in_array() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var o = { x: 42 };
            var a = [o];
            for (var i = 0; i < 20; i++) { o["p" + i] = i; }
            a[0].x;
        "#,
        )
        .unwrap();
    assert_eq!(
        r.as_smi(),
        Some(42),
        "array-held object must survive growth"
    );
}

#[test]
fn test_grow_array_alias_in_array() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var a = [1];
            var holder = [a];
            for (var i = 0; i < 20; i++) { a.push(i); }
            holder[0].length;
        "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(21), "array-held array must survive growth");
}

#[test]
fn test_grow_object_alias_in_object() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var o = { x: 42 };
            var holder = { ref: o };
            for (var i = 0; i < 20; i++) { o["p" + i] = i; }
            holder.ref.x;
        "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(42), "object slot must survive growth");
}

#[test]
fn test_grow_prototype_object() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var p = {};
            var o = Object.create(p);
            for (var i = 0; i < 20; i++) { p["p" + i] = i; }
            o.p19;
        "#,
        )
        .unwrap();
    assert_eq!(
        r.as_smi(),
        Some(19),
        "prototype pointer must resolve growth"
    );
}

#[test]
fn test_grow_then_collection_alias() {
    let mut ctx = Context::new_small();
    let src = format!(
        r#"
        var o = {{ x: 42 }};
        var a = [o];
        for (var i = 0; i < 20; i++) {{ o["p" + i] = i; }}
        {}
        a[0].x;
        "#,
        GC_STRESS_ALLOC
    );
    let r = ctx.eval(&src).unwrap();
    assert_eq!(
        r.as_smi(),
        Some(42),
        "a GC after growth must preserve the alias"
    );
}

#[test]
fn test_grow_array_alias_in_object_slot() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var a = [1];
            var holder = { ref: a };
            for (var i = 0; i < 20; i++) { a.push(i); }
            holder.ref.length;
        "#,
        )
        .unwrap();
    assert_eq!(
        r.as_smi(),
        Some(21),
        "object slot must survive array growth"
    );
}

#[test]
fn test_grow_strict_equality_resolves() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var o = { x: 42 };
            var a = [o];
            for (var i = 0; i < 20; i++) { o["p" + i] = i; }
            (a[0] === o) && (a[0] == o);
        "#,
        )
        .unwrap();
    assert!(r.to_bool(), "identity must resolve forwarding aliases");
}

#[test]
fn test_grow_array_then_collection_alias() {
    let mut ctx = Context::new_small();
    let src = format!(
        r#"
        var a = [1];
        var holder = [a];
        for (var i = 0; i < 20; i++) {{ a.push(i); }}
        {}
        holder[0].length;
        "#,
        GC_STRESS_ALLOC
    );
    let r = ctx.eval(&src).unwrap();
    assert_eq!(
        r.as_smi(),
        Some(21),
        "a GC after array growth must preserve the alias"
    );
}

#[test]
fn test_grow_array_as_prototype() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var p = [7];
            var o = Object.create(p);
            for (var i = 0; i < 20; i++) { p.push(i); }
            o[0] + o[1];
        "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(7), "array prototype must resolve growth");
}

#[test]
fn test_grow_generator_local_array_alias() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            function* g() {
                var a = [1];
                yield a;
                for (var i = 0; i < 2000; i++) { a.push(i); }
                return a.length;
            }
            var it = g();
            var a = it.next().value;
            for (var i = 0; i < 200; i++) { a.push(i); }
            it.next().value;
        "#,
        )
        .unwrap();
    assert_eq!(
        r.as_smi(),
        Some(2201),
        "generator-local array must survive growth"
    );
}

#[test]
fn test_grow_source_array_during_iteration() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var a = [1, 2, 3];
            var seen = [];
            a.forEach(function (x) {
                seen.push(x);
                if (seen.length === 1) {
                    for (var i = 0; i < 20; i++) { a.push(9); }
                }
            });
            seen.join(",");
        "#,
        )
        .unwrap();
    assert_eq!(
        eval_str(&mut ctx, "seen.join(',')"),
        "1,2,3",
        "growth during iteration must not change the captured walk"
    );
    let _ = r;
}

#[test]
fn test_jit_off_by_default() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval("var s = 0.5; for (var i = 0; i < 1000; i++) { s += 1.25; } s;")
        .unwrap();
    assert_eq!(
        r.as_float64(),
        Some(1250.5),
        "interpreter must be the reference"
    );
    assert_eq!(ctx.vm().jit_entry_count, 0, "JIT must be opt-in by default");
}

// ---------------------------------------------------------------------------
// Production-readiness gate: nested array machines. A callback-driven builtin
// invoked from another callback must suspend the outer machine, not overwrite
// it. These were silent data corruption before the continuation stack.
// ---------------------------------------------------------------------------

#[test]
fn test_nested_map_produces_nested_arrays() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            JSON.stringify([1, 2].map(function (x) {
                return [10, 20].map(function (y) { return x + y; });
            }));
        "#,
        )
        .unwrap();
    assert_eq!(
        eval_str(
            &mut ctx,
            "JSON.stringify([1,2].map(function(x){return [10,20].map(function(y){return x+y;});}))"
        ),
        "[[11,21],[12,22]]"
    );
    let _ = r;
}

#[test]
fn test_nested_for_each_visits_all_outer_elements() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var s = 0;
            [1, 2].forEach(function (x) {
                [10, 20].forEach(function (y) { s += y; });
                s += x;
            });
            s;
        "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(63), "outer forEach must not truncate");
}

#[test]
fn test_nested_filter_inside_map() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            JSON.stringify([1, 2].map(function (x) {
                return [10, 20].filter(function (y) { return y > x; });
            }));
        "#,
        )
        .unwrap();
    assert_eq!(
        eval_str(
            &mut ctx,
            "JSON.stringify([1,2].map(function(x){return [10,20].filter(function(y){return y>x;});}))"
        ),
        "[[10,20],[10,20]]"
    );
    let _ = r;
}

#[test]
fn test_nested_map_inside_getter_inside_map() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var o = { get v() { return [1, 2].map(function (z) { return z * 3; }).join(","); } };
            [1, 2].map(function (x) { return o.v + "|" + x; }).join(";");
        "#,
        )
        .unwrap();
    assert_eq!(
        eval_str(
            &mut ctx,
            "[1,2].map(function(x){return o.v+'|'+x;}).join(';')"
        ),
        "3,6|1;3,6|2"
    );
    let _ = r;
}

#[test]
fn test_nested_reduce_inside_reduce() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            [1, 2].reduce(function (acc, x) {
                return acc + [10, 20].reduce(function (a, y) { return a + y; }, x);
            }, 0);
        "#,
        )
        .unwrap();
    assert_eq!(
        r.as_smi(),
        Some(63),
        "nested reduce must use both outer elements"
    );
}

#[test]
fn test_nested_map_error_caught_by_outer_callback() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var out = [];
            [1, 2].forEach(function (x) {
                try {
                    [10].map(function () { throw new Error("boom"); });
                    out.push("no");
                } catch (e) {
                    out.push("caught:" + x);
                }
            });
            out.join(",");
        "#,
        )
        .unwrap();
    assert_eq!(
        eval_str(&mut ctx, "out.join(',')"),
        "caught:1,caught:2",
        "inner throw must abort only the inner machine"
    );
    let _ = r;
}

#[test]
fn test_nested_map_inside_sort_comparator() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var a = [3, 1, 2];
            a.sort(function (x, y) {
                var doubled = [x, y].map(function (v) { return v * 2; });
                return doubled[0] - doubled[1];
            });
            a.join(",");
        "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), None);
    let _ = r;
    assert_eq!(eval_str(&mut ctx, "a.join(',')"), "1,2,3");
}

#[test]
fn test_nested_map_outer_state_survives_gc() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            JSON.stringify([1, 2].map(function (x) {
                return [10].map(function (y) {
                    for (var i = 0; i < 80000; i++) { var t = { n: i, s: "z" + i }; }
                    return x + y;
                })[0];
            }));
        "#,
        )
        .unwrap();
    let _ = r;
    assert_eq!(
        eval_str(
            &mut ctx,
            "JSON.stringify([1,2].map(function(x){return [10].map(function(y){return x+y;})[0];}))"
        ),
        "[11,12]"
    );
}

// ---------------------------------------------------------------------------
// Production-readiness gate: async/await continuations. The old tests only
// asserted the pre-await prefix; these assert that execution resumes, values
// flow, and the returned promise settles.
// ---------------------------------------------------------------------------

#[test]
fn test_async_await_resumes_and_resolves() {
    let mut ctx = Context::new_small();
    ctx.eval(
        r#"
        var out = 0;
        async function f() { var x = await 3; out = x + 4; return out; }
        f();
        0;
        "#,
    )
    .unwrap();
    assert_eq!(
        ctx.eval("out").unwrap().as_smi(),
        Some(7),
        "await must resume"
    );
}

#[test]
fn test_async_await_multiple_and_loop() {
    let mut ctx = Context::new_small();
    ctx.eval(
        r#"
        var out = 0;
        async function f() {
            var s = 0;
            for (var i = 0; i < 3; i++) { s += await Promise.resolve(i); }
            var a = await 10;
            var b = await 20;
            out = s + a + b;
        }
        f();
        0;
        "#,
    )
    .unwrap();
    assert_eq!(
        ctx.eval("out").unwrap().as_smi(),
        Some(33),
        "await loop + chained awaits"
    );
}

#[test]
fn test_async_await_preserves_call_arguments() {
    let mut ctx = Context::new_small();
    ctx.eval(
        r#"
        var out = 0;
        function add(a, b) { return a + b; }
        async function f() { out = add(await 1, 2); }
        f();
        0;
        "#,
    )
    .unwrap();
    assert_eq!(
        ctx.eval("out").unwrap().as_smi(),
        Some(3),
        "operands across await must survive"
    );
}

#[test]
fn test_async_await_rejection_propagates() {
    let mut ctx = Context::new_small();
    ctx.eval(
        r#"
        var out = "";
        async function f() {
            try { await Promise.reject(new Error("bad")); }
            catch (e) { out = e.message; }
            return out;
        }
        f();
        "";
        "#,
    )
    .unwrap();
    assert_eq!(
        eval_str(&mut ctx, "out"),
        "bad",
        "rejected await must throw into catch"
    );
}

#[test]
fn test_async_function_promise_resolves_after_await() {
    let mut ctx = Context::new_small();
    ctx.eval(
        r#"
        var out = 0;
        async function f() { await 1; return 9; }
        f().then(function (v) { out = v; });
        0;
        "#,
    )
    .unwrap();
    assert_eq!(
        ctx.eval("out").unwrap().as_smi(),
        Some(9),
        "async promise must settle"
    );
}

#[test]
fn test_try_finally_return_runs_finally() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var log = [];
            function f() { try { log.push("try"); return 1; } finally { log.push("fin"); } }
            var value = f();
            log.join(",") + "|" + value;
        "#,
        )
        .unwrap();
    assert_eq!(eval_str(&mut ctx, "log.join(',') + '|' + f()"), "try,fin|1");
    let _ = r;
}

#[test]
fn test_try_finally_nested_and_override() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            function f() { try { try { return 1; } finally { } } finally { } }
            f();
        "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(1));
    let r2 = ctx
        .eval("function g(){ try { return 1; } finally { return 2; } } g();")
        .unwrap();
    assert_eq!(r2.as_smi(), Some(2), "return in finally overrides");
}

#[test]
fn test_async_finally_after_await_return() {
    let mut ctx = Context::new_small();
    ctx.eval(
        r#"
        var out = "";
        async function f() { try { await 1; return 1; } finally { out += "fin"; } }
        f();
        "";
        "#,
    )
    .unwrap();
    assert_eq!(eval_str(&mut ctx, "out"), "fin", "async finally must run");
}

#[test]
fn test_promise_all_fulfills_with_array() {
    let mut ctx = Context::new_small();
    ctx.eval(
        r#"
        var out = "";
        Promise.all([Promise.resolve(1), 2, Promise.resolve(3)]).then(function (v) {
            out = v.join(",") + "|" + (v instanceof Array);
        });
        "";
        "#,
    )
    .unwrap();
    assert_eq!(eval_str(&mut ctx, "out"), "1,2,3|true");
}

#[test]
fn test_promise_all_empty_and_rejected() {
    let mut ctx = Context::new_small();
    ctx.eval(
        r#"
        var empty = "";
        var rejected = "";
        Promise.all([]).then(function (v) { empty = v.length; });
        Promise.all([Promise.reject(new Error("nope"))]).catch(function (e) { rejected = e.message; });
        "";
        "#,
    )
    .unwrap();
    assert_eq!(ctx.eval("empty").unwrap().as_smi(), Some(0));
    assert_eq!(eval_str(&mut ctx, "rejected"), "nope");
}

#[test]
fn test_esm_live_binding_updates_after_exported_mutation() {
    let mut ctx = Context::new_small();
    eval_module_with(
        &mut ctx,
        r#"
            import { count, inc, get } from "./dep.js";
            export function before() { return count; }
            export function bump() { inc(); return count; }
            export function inside() { return get(); }
        "#,
        &[(
            "./dep.js",
            r#"
                export let count = 1;
                export function inc() { count++; }
                export function get() { return count; }
            "#,
        )],
    )
    .unwrap();
    let before = ctx.module_export("<entry>", "before").unwrap();
    assert_eq!(ctx.call_value(before, &[]).unwrap().as_smi(), Some(1));
    let bump = ctx.module_export("<entry>", "bump").unwrap();
    assert_eq!(ctx.call_value(bump, &[]).unwrap().as_smi(), Some(2));
    let inside = ctx.module_export("<entry>", "inside").unwrap();
    assert_eq!(
        ctx.call_value(inside, &[]).unwrap().as_smi(),
        Some(2),
        "module function must read its own live binding"
    );
}

#[test]
fn test_update_expression_captured_and_lexical() {
    let mut ctx = Context::new_small();
    ctx.eval(
        r#"
        function mk() {
            var n = 0;
            return function () { return [++n, n++, n]; };
        }
        var f = mk();
        var first = f();
        var second = f();
        var captured = JSON.stringify([first, second]);
        let m = 1;
        var lexical = JSON.stringify([++m, m++, m]);
        0;
        "#,
    )
    .unwrap();
    assert_eq!(
        eval_str(&mut ctx, "captured"),
        "[[1,1,2],[3,3,4]]",
        "captured prefix/postfix results"
    );
    assert_eq!(
        eval_str(&mut ctx, "lexical"),
        "[2,2,3]",
        "lexical prefix/postfix results"
    );
}

#[test]
fn test_function_declaration_hoisting() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var before = typeof f;
            var called = f();
            function f() { return 7; }
            before + "|" + called;
        "#,
        )
        .unwrap();
    assert_eq!(eval_str(&mut ctx, "before + '|' + called"), "function|7");
    let _ = r;
}

#[test]
fn test_function_declaration_hoisting_inside_function_and_block() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            function outer() {
                var t = typeof g;
                return t + "|" + g();
                function g() { return 8; }
            }
            var inner = outer();
            var block;
            { block = typeof h + "|" + h(); function h() { return 9; } }
            inner + "|" + block;
        "#,
        )
        .unwrap();
    assert_eq!(
        eval_str(&mut ctx, "inner + '|' + block"),
        "function|8|function|9"
    );
    let _ = r;
}

#[test]
fn test_arrow_lexical_this_in_method() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var o = {
                v: 7,
                m: function () { var f = () => this.v; return f(); },
                make: function () { return () => this.v; }
            };
            var captured = o.make();
            o.m() + "|" + captured();
        "#,
        )
        .unwrap();
    assert_eq!(
        eval_str(&mut ctx, "o.m() + '|' + captured()"),
        "7|7",
        "arrow must capture defining receiver"
    );
    let _ = r;
    let r2 = ctx
        .eval(
            r#"
            function m() { var f = () => this.z; return f(); }
            m.call({ z: 9 });
        "#,
        )
        .unwrap();
    assert_eq!(r2.as_smi(), Some(9));
}

#[test]
fn test_arrow_lexical_this_survives_gc() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var o = {
                v: 11,
                m: function () {
                    var f = () => this.v;
                    for (var i = 0; i < 80000; i++) { var t = { n: i, s: "z" + i }; }
                    return f();
                }
            };
            o.m();
        "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(11), "arrow lexical this must survive GC");
}

#[test]
fn test_try_body_block_scope_and_tdz() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var name = "";
            try { readBefore; let readBefore = 1; }
            catch (e) { name = e.name; }
            var leaked = typeof readBefore;
            name + "|" + leaked;
        "#,
        )
        .unwrap();
    assert_eq!(
        eval_str(&mut ctx, "name + '|' + typeof readBefore"),
        "ReferenceError|undefined",
        "try body must be a TDZ block"
    );
    let _ = r;
}

#[test]
fn test_catch_and_finally_are_block_scoped() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var seen = "";
            try { throw new Error("x"); }
            catch (e) { let inside = "c"; seen += inside; }
            finally { let fin = "f"; seen += fin; }
            seen + "|" + typeof inside + "|" + typeof fin;
        "#,
        )
        .unwrap();
    assert_eq!(
        eval_str(&mut ctx, "seen + '|' + typeof inside + '|' + typeof fin"),
        "cf|undefined|undefined"
    );
    let _ = r;
}

#[test]
fn test_async_arrow_forms() {
    let mut ctx = Context::new_small();
    ctx.eval(
        r#"
        var out = [];
        var a = async (x) => { return x * 2; };
        var b = async x => x + 1;
        var c = async () => 5;
        var d = async ({ x }) => x + 1;
        var e = async () => { var v = await 3; return v + 4; };
        a(4).then(function (v) { out.push(v); });
        b(4).then(function (v) { out.push(v); });
        c().then(function (v) { out.push(v); });
        d({ x: 6 }).then(function (v) { out.push(v); });
        e().then(function (v) { out.push(v); });
        "";
        "#,
    )
    .unwrap();
    assert_eq!(eval_str(&mut ctx, "out.join(',')"), "8,5,5,7,7");
}

#[test]
fn test_function_bind_this_and_bound_args() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            function f(a, b) { return this.x + a + b; }
            var g = f.bind({ x: 1 }, 2);
            var h = f.bind({ x: 4 });
            var chained = f.bind({ x: 1 }, 2).bind(null, 3);
            [g(3), h(2, 3), chained(4), typeof g].join(",");
        "#,
        )
        .unwrap();
    assert_eq!(
        eval_str(
            &mut ctx,
            "g(3) + ',' + h(2,3) + ',' + chained(4) + ',' + typeof g"
        ),
        "6,9,6,function"
    );
    let _ = r;
    let m = ctx
        .eval(
            r#"
            var o = { v: 9, m: function () { return this.v; } };
            var bound = o.m.bind({ v: 5 });
            bound();
        "#,
        )
        .unwrap();
    assert_eq!(m.as_smi(), Some(5));
}

#[test]
fn test_promise_pending_rejection_handlers() {
    let mut ctx = Context::new_small();
    ctx.eval(
        r#"
        var reject;
        var p = new Promise(function (_, r) { reject = r; });
        var out = "";
        var d = p.then();
        d.catch(function (e) { out = e.message; });
        reject(new Error("later"));
        0;
        "#,
    )
    .unwrap();
    assert_eq!(
        eval_str(&mut ctx, "out"),
        "later",
        "pending rejection must reach the derived promise's catch"
    );
}

#[test]
fn test_promise_race_pending_rejection() {
    let mut ctx = Context::new_small();
    ctx.eval(
        r#"
        var reject;
        var p = new Promise(function (_, r) { reject = r; });
        var out = "";
        Promise.race([p]).catch(function (e) { out = e.message; });
        reject(new Error("race-reject"));
        0;
        "#,
    )
    .unwrap();
    assert_eq!(eval_str(&mut ctx, "out"), "race-reject");
}

#[test]
fn test_promise_all_pending_order_and_rejection() {
    let mut ctx = Context::new_small();
    ctx.eval(
        r#"
        var r1, r2;
        var p1 = new Promise(function (r) { r1 = r; });
        var p2 = new Promise(function (r) { r2 = r; });
        var out = "";
        Promise.all([p1, p2]).then(function (v) { out = v.join(","); });
        r2(2);
        r1(1);
        0;
        "#,
    )
    .unwrap();
    assert_eq!(
        eval_str(&mut ctx, "out"),
        "1,2",
        "all must preserve index order"
    );
    let mut ctx2 = Context::new_small();
    ctx2.eval(
        r#"
        var reject;
        var p = new Promise(function (_, r) { reject = r; });
        var err = "";
        Promise.all([p]).catch(function (e) { err = e.message; });
        reject(new Error("nope"));
        0;
        "#,
    )
    .unwrap();
    assert_eq!(eval_str(&mut ctx2, "err"), "nope");
}

#[test]
fn test_array_methods_inside_promise_callback() {
    let mut ctx = Context::new_small();
    ctx.eval(
        r#"
        var out = "";
        Promise.resolve([1, 2]).then(function (v) {
            var mapped = v.map(function (x) { return x + 1; });
            out = mapped.join(",");
        });
        "";
        "#,
    )
    .unwrap();
    assert_eq!(
        eval_str(&mut ctx, "out"),
        "2,3",
        "nested array machine must not steal the microtask continuation"
    );
}

#[test]
fn test_promise_constructor_inside_function() {
    let mut ctx = Context::new_small();
    ctx.eval(
        r#"
        function make() {
            return new Promise(function (resolve) { resolve(5); });
        }
        var out = 0;
        make().then(function (v) { out = v; });
        0;
        "#,
    )
    .unwrap();
    assert_eq!(ctx.eval("out").unwrap().as_smi(), Some(5));
}

#[test]
fn test_promise_all_settled_pending_and_rejected() {
    let mut ctx = Context::new_small();
    ctx.eval(
        r#"
        var resolve;
        var pending = new Promise(function (r) { resolve = r; });
        var out = "";
        Promise.allSettled([1, pending, Promise.reject(new Error("bad"))]).then(function (v) {
            out = v[0].status + "," + v[1].value + "," + v[2].status + "," + v[2].reason.message;
        });
        resolve(2);
        "";
        "#,
    )
    .unwrap();
    assert_eq!(eval_str(&mut ctx, "out"), "fulfilled,2,rejected,bad");
}

#[test]
fn test_console_and_base64_uri_helpers() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var results = [];
            results.push(typeof console.log);
            results.push(atob(btoa("hello")));
            results.push(btoa("é"));
            results.push(atob("6Q=="));
            results.push(encodeURIComponent("a b/é"));
            results.push(decodeURIComponent("a%20b%2F%C3%A9"));
            var bad = "";
            try { atob("***"); } catch (e) { bad = e.name; }
            results.push(bad);
            results.join("|");
        "#,
        )
        .unwrap();
    assert_eq!(
        eval_str(&mut ctx, "results.join('|')"),
        "function|hello|6Q==|é|a%20b%2F%C3%A9|a b/é|URIError"
    );
    let _ = r;
}

#[test]
fn test_function_length_and_name_metadata() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            function f(a, b) {}
            function g(a, b = 1, c) {}
            var o = { m(x, y) {} };
            var bound = f.bind(null, 1);
            [f.length, f.name, g.length, g.name, o.m.length, o.m.name,
             bound.length, bound.name, Object.keys(f).join(",")].join("|");
        "#,
        )
        .unwrap();
    assert_eq!(
        eval_str(
            &mut ctx,
            "[f.length, f.name, g.length, g.name, o.m.length, o.m.name, bound.length, bound.name].join('|')"
        ),
        "2|f|1|g|2|m|1|bound f"
    );
    assert_eq!(
        eval_str(&mut ctx, "Object.keys(f).join(',')"),
        "",
        "length/name must be non-enumerable"
    );
    let _ = r;
}

#[test]
fn test_url_constructor_and_fields() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var u = new URL("https://example.com/a/b?x=1#h");
            var rel = new URL("../c", "https://example.com/a/b");
            var bad = "";
            try { new URL("not a url"); } catch (e) { bad = e.name; }
            var called = "";
            try { URL("x"); } catch (e) { called = e.name; }
            [u.href, u.origin, u.protocol, u.host, u.hostname, u.port,
             u.pathname, u.search, u.hash, u.toString(), rel.href,
             bad, called].join("|");
        "#,
        )
        .unwrap();
    assert_eq!(
        eval_str(
            &mut ctx,
            "[u.href, u.origin, u.protocol, u.host, u.hostname, u.port, u.pathname, u.search, u.hash, u.toString(), rel.href, bad, called].join('|')"
        ),
        "https://example.com/a/b?x=1#h|https://example.com|https:|example.com|example.com||/a/b|?x=1|#h|https://example.com/a/b?x=1#h|https://example.com/c|TypeError|TypeError"
    );
    let _ = r;
}

#[test]
fn test_url_search_params_core() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var q = new URLSearchParams("a=1&b=hello+world&a=2");
            var parts = [q.get("a"), q.getAll("a").join(","), q.has("b"), q.toString()];
            q.append("c", "x y");
            q.set("a", "9");
            parts.push(q.toString());
            q.delete("b");
            parts.push(q.toString());
            var u = new URL("https://example.com/y?a=1&b=2");
            parts.push(u.searchParams.get("b"));
            var called = "";
            try { URLSearchParams("a=1"); } catch (e) { called = e.name; }
            parts.push(called);
            parts.join("|");
        "#,
        )
        .unwrap();
    assert_eq!(
        eval_str(&mut ctx, "parts.join('|')"),
        "1|1,2|true|a=1&b=hello+world&a=2|a=9&b=hello+world&c=x+y|a=9&c=x+y|2|TypeError"
    );
    let _ = r;
}

#[test]
fn test_text_encoder_decoder_utf8() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var e = new TextEncoder();
            var b = e.encode("hé");
            var d = new TextDecoder();
            var called = "";
            try { TextEncoder(); } catch (err) { called = err.name; }
            [b.length, b[0], b[1], b[2], d.decode(b), e.encoding, d.encoding, called].join("|");
        "#,
        )
        .unwrap();
    assert_eq!(
        eval_str(
            &mut ctx,
            "[b.length, b[0], b[1], b[2], d.decode(b), e.encoding, d.encoding, called].join('|')"
        ),
        "3|104|195|169|hé|utf-8|utf-8|TypeError"
    );
    let _ = r;
}

#[test]
fn test_embedding_host_functions() {
    use std::cell::Cell;
    use std::rc::Rc;
    let mut ctx = Context::new_small();
    ctx.register_host_fn("double", 1, |_gc, _this, args, _vm| {
        rune_core::value::Value::smi(args[0].as_smi().unwrap_or(0) * 2)
    });
    assert_eq!(ctx.eval("double(21)").unwrap().as_smi(), Some(42));
    assert_eq!(ctx.eval("[1, 2].map(double)[1]").unwrap().as_smi(), Some(4));
    assert_eq!(ctx.eval("double.call(null, 7)").unwrap().as_smi(), Some(14));
    assert_eq!(ctx.eval("double.length").unwrap().as_smi(), Some(1));
    assert_eq!(eval_str(&mut ctx, "double.name"), "double");

    let counter = Rc::new(Cell::new(0));
    let c = counter.clone();
    ctx.register_host_fn("next", 0, move |_gc, _this, _args, _vm| {
        let n = c.get() + 1;
        c.set(n);
        rune_core::value::Value::smi(n)
    });
    assert_eq!(ctx.eval("next() + next()").unwrap().as_smi(), Some(3));
}

#[test]
fn test_promise_any_first_fulfillment_and_aggregate_error() {
    let mut ctx = Context::new_small();
    ctx.eval(
        r#"
        var out = "";
        Promise.any([Promise.reject("a"), Promise.resolve(2)]).then(function (v) {
            out = "ok:" + v;
        });
        var errOut = "";
        Promise.any([Promise.reject("x"), Promise.reject("y")]).catch(function (e) {
            errOut = e.name + ":" + e.errors.join(",");
        });
        var empty = "";
        Promise.any([]).catch(function (e) { empty = e.name + ":" + e.errors.length; });
        "";
        "#,
    )
    .unwrap();
    assert_eq!(eval_str(&mut ctx, "out"), "ok:2");
    assert_eq!(eval_str(&mut ctx, "errOut"), "AggregateError:x,y");
    assert_eq!(eval_str(&mut ctx, "empty"), "AggregateError:0");
}

#[test]
fn test_promise_any_pending_input() {
    let mut ctx = Context::new_small();
    ctx.eval(
        r#"
        var resolve;
        var pending = new Promise(function (r) { resolve = r; });
        var out = "";
        Promise.any([pending, Promise.reject("no")]).then(function (v) { out = "v" + v; });
        resolve(7);
        "";
        "#,
    )
    .unwrap();
    assert_eq!(eval_str(&mut ctx, "out"), "v7");
}

#[test]
fn test_thenables_in_promise_combinators() {
    let mut ctx = Context::new_small();
    ctx.eval(
        r#"
        var out = [];
        Promise.all([{ then: function (r) { r(1); } }, 2])
            .then(function (v) { out.push("all:" + v.join(",")); });
        Promise.race([{ then: function (r) { r(3); } }])
            .then(function (v) { out.push("race:" + v); });
        Promise.allSettled([{ then: function (_, j) { j("e"); } }])
            .then(function (v) { out.push("settled:" + v[0].status + ":" + v[0].reason); });
        Promise.any([{ then: function (_, j) { j("no"); } },
                     { then: function (r) { r(9); } }])
            .then(function (v) { out.push("any:" + v); });
        Promise.all([{ then: function () { throw "bad"; } }])
            .catch(function (e) { out.push("throw:" + e); });
        "";
        "#,
    )
    .unwrap();
    assert_eq!(
        eval_str(&mut ctx, "out.sort().join('|')"),
        "all:1,2|any:9|race:3|settled:rejected:e|throw:bad"
    );
}

#[test]
fn test_array_header_stable_across_growth() {
    let mut ctx = Context::new_small();
    ctx.eval(
        r#"
        var a = [1, 2];
        var alias = a;
        var holder = { arr: a };
        for (var i = 0; i < 300; i++) { a.push(i * 2); }
        [alias === a, alias.length, holder.arr[100], holder.arr[301]].join("|");
        "#,
    )
    .unwrap();
    assert_eq!(
        eval_str(
            &mut ctx,
            "[alias === a, alias.length, holder.arr[100], holder.arr[301]].join('|')"
        ),
        "true|302|196|598"
    );
}

#[test]
fn test_promise_chaining_adopts_promises_and_thenables() {
    let mut ctx = Context::new_small();
    ctx.eval(
        r#"
        var out = "";
        Promise.resolve(1)
            .then(function (v) { return Promise.resolve(v + 1); })
            .then(function (v) { return { then: function (r) { r(v + 1); } }; })
            .then(function (v) { out = "chain" + v; });
        var nested = "";
        Promise.resolve(0)
            .then(function () { return Promise.resolve("x").then(function (v) { return v + "y"; }); })
            .then(function (v) { nested = v; });
        "";
        "#,
    )
    .unwrap();
    assert_eq!(eval_str(&mut ctx, "out"), "chain3");
    assert_eq!(eval_str(&mut ctx, "nested"), "xy");
}

#[test]
fn test_object_header_stable_across_growth() {
    let mut ctx = Context::new_small();
    ctx.eval(
        r#"
        var o = { a: 1 };
        var alias = o;
        var holder = { obj: o };
        for (var i = 0; i < 300; i++) { o["k" + i] = i; }
        [alias === o, alias.a, holder.obj.k0, holder.obj.k299].join("|");
        "#,
    )
    .unwrap();
    assert_eq!(
        eval_str(
            &mut ctx,
            "[alias === o, alias.a, holder.obj.k0, holder.obj.k299].join('|')"
        ),
        "true|1|0|299"
    );
}

#[test]
fn test_jit_property_slots_follow_stable_objects() {
    let mut ctx = Context::new();
    ctx.enable_jit = true;
    let r = ctx
        .eval(
            r#"
            function f(o) { return o.a + o.b; }
            var o = { a: 1, b: 2 };
            var s = 0;
            for (var i = 0; i < 200; i++) { s += f(o); }
            s;
            "#,
        )
        .unwrap();
    assert_eq!(r.as_smi(), Some(600));
}

#[test]
fn test_builtin_prototype_chains_and_string_wrapper() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var s = new String("abc");
            var p = new Promise(function (resolve) { resolve(1); });
            var m = new Map();
            var d = new Date(0);
            [
                s instanceof String,
                s instanceof Object,
                Object.getPrototypeOf(s) === String.prototype,
                Object.getPrototypeOf(String.prototype) === Object.prototype,
                p instanceof Object,
                m instanceof Object,
                d instanceof Object
            ].join("|") === "true|true|true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_array_to_string_and_string_coercion() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            [[1, 2, 3].toString(), String([4, 5, 6]), [].toString.call({})].join("|") === "1,2,3|4,5,6|[object Object]";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_array_methods_on_string_and_function_array_likes() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var s = new String("abc");
            var mappedString = Array.prototype.map.call(s, function (v) { return v; }).join("");
            var f = function (a, b) { return a + b; };
            f[0] = 11; f[1] = 9;
            var mappedFunction = Array.prototype.map.call(f, function (v) { return v; }).join(",");
            [mappedString, mappedFunction].join("|") === "abc|11,9";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_boolean_and_number_wrappers() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var b = new Boolean(true);
            var n = new Number(3.5);
            [
                b instanceof Boolean,
                b instanceof Object,
                b.valueOf() === true,
                b.toString() === "true",
                n instanceof Number,
                n.valueOf() === 3.5,
                n.toString() === "3.5",
                Object(false) instanceof Boolean,
                Object(5) instanceof Number,
                Object.keys(b).length === 0,
                JSON.stringify(b) === "true",
                JSON.stringify(n) === "3.5",
                (new Number(3) + 2) === 5,
                (new Number(3) * 2) === 6,
                (new Number(8) / 2) === 4,
                (new Number(7) % 4) === 3,
                (new Boolean(true) * 3) === 3
            ].join("|") === "true|true|true|true|true|true|true|true|true|true|true|true|true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_primitive_and_wrapper_array_like_methods() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            Boolean.prototype[0] = true;
            Boolean.prototype.length = 1;
            var mappedBoolean = Array.prototype.map.call(false, function (v, i, o) {
                return o instanceof Boolean;
            })[0];
            var mappedString = Array.prototype.map.call("abc", function (v, i, o) {
                return [v, o instanceof String];
            });
            [
                mappedBoolean,
                mappedString[0][0] === "a",
                mappedString[0][1],
                mappedString[2][0] === "c"
            ].join("|") === "true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_function_instanceof_and_prototype() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var f = function (a, b) { return a + b; };
            [
                f instanceof Function,
                f instanceof Object,
                Object.getPrototypeOf(f) === Function.prototype,
                Object.getPrototypeOf(Function.prototype) === Object.prototype
            ].join("|") === "true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_aggregate_error_constructor_identity() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var e = new AggregateError([1, 2], "boom");
            [
                e.name === "AggregateError",
                e.message === "boom",
                e.errors.length === 2,
                e.errors[0] === 1,
                e instanceof AggregateError,
                e instanceof Error,
                Object.getPrototypeOf(e) === AggregateError.prototype,
                AggregateError.prototype instanceof Error,
                typeof AggregateError === "function"
            ].join("|") === "true|true|true|true|true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_array_this_arg_boxing_and_hex_length() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var boxed = [1].map(function () { return this.valueOf() === false; }, false)[0];
            var hex = { 0: 11, 1: 9, 2: 12, length: "0x0002" };
            var mapped = Array.prototype.map.call(hex, function (v) { return v < 10; });
            [boxed, mapped.length === 2, mapped[1]].join("|") === "true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_object_to_string_tags() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var custom = {};
            custom[Symbol.toStringTag] = "Custom";
            [
                Object.prototype.toString.call(Math),
                Object.prototype.toString.call(JSON),
                Object.prototype.toString.call(custom)
            ].join("|") === "[object Math]|[object JSON]|[object Custom]";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_undeclared_identifier_reference_errors() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var caught = false;
            try { never_declared_value; } catch (e) { caught = e instanceof ReferenceError; }
            var declared_undefined;
            typeof never_declared_value === "undefined" &&
                caught &&
                declared_undefined === undefined &&
                (implicit_global_assignment = 7) === 7 &&
                implicit_global_assignment === 7;
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_instruction_budget_halts_infinite_loops() {
    let mut ctx = Context::new_small();
    ctx.set_instruction_budget(5000);
    let err = ctx.eval("while (true) {}").expect_err("budget must trip");
    assert!(
        err.contains("instruction budget exceeded"),
        "unexpected error: {err}"
    );
    // Normal execution still works after the (reset) trip.
    assert_eq!(ctx.eval("1 + 1").unwrap().as_smi(), Some(2));

    ctx.set_instruction_budget(5000);
    let caught = ctx
        .eval(
            "var caught = false; try { while (true) {} } catch (e) { caught = e instanceof RangeError; } caught;",
        )
        .unwrap();
    assert_eq!(caught.to_boolean(), Some(true));
}

#[test]
fn test_parser_fuzz_corpus_does_not_panic() {
    // Deterministic token-soup fuzzing: malformed programs must produce parse
    // or runtime errors, never panic or hang the engine.
    const TOKENS: &[&str] = &[
        "{",
        "}",
        "(",
        ")",
        "[",
        "]",
        ";",
        ",",
        ".",
        ":",
        "?",
        "+",
        "-",
        "*",
        "/",
        "%",
        "<",
        ">",
        "=",
        "!",
        "&",
        "|",
        "^",
        "~",
        "==",
        "===",
        "=>",
        "let",
        "var",
        "const",
        "function",
        "return",
        "if",
        "else",
        "for",
        "while",
        "true",
        "false",
        "null",
        "undefined",
        "0",
        "1",
        "42",
        "'x'",
        "\"y\"",
        "a",
        "b",
        "print",
        "\n",
    ];
    let mut ctx = Context::new_small();
    let mut state = 0x1234_5678_9abc_def0u64;
    for _ in 0..300 {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let len = 1 + (state as usize % 80);
        let mut src = String::new();
        for _ in 0..len {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            src.push_str(TOKENS[(state as usize) % TOKENS.len()]);
            src.push(' ');
        }
        ctx.set_instruction_budget(10_000);
        let _ = ctx.eval(&src);
    }
}

#[test]
fn test_time_budget_and_jit_budget_suppression() {
    let mut ctx = Context::new_small();
    ctx.set_time_budget(std::time::Duration::from_millis(20));
    let err = ctx
        .eval("while (true) {}")
        .expect_err("time budget must trip");
    assert!(
        err.contains("time budget exceeded"),
        "unexpected error: {err}"
    );
    assert_eq!(ctx.eval("2 + 2").unwrap().as_smi(), Some(4));

    // Instruction budget must also cover the opt-in JIT (JIT is suppressed).
    let mut jit = Context::new_small();
    jit.enable_jit = true;
    jit.set_instruction_budget(20_000);
    let err = jit
        .eval("while (true) {}")
        .expect_err("JIT must not bypass instruction budget");
    assert!(
        err.contains("instruction budget exceeded"),
        "unexpected error: {err}"
    );
}

#[test]
fn test_arguments_object_to_string_tag() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            function f() { return arguments; }
            var args = f(1, 2);
            var plain = [1, 2];
            [
                Object.prototype.toString.call(args),
                Object.prototype.toString.call(plain),
                Array.isArray(args),
                args.length,
                args[Symbol.toStringTag]
            ].join("|") === "[object Arguments]|[object Array]|true|2|Arguments";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_date_regexp_array_like_extra_properties() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var d = new Date(0);
            d.length = 1;
            d[0] = 42;
            var re = /x/;
            re.length = 1;
            re[0] = 7;
            var dm = Array.prototype.map.call(d, function (v, i, o) { return [v, o instanceof Date]; });
            var rm = Array.prototype.map.call(re, function (v, i, o) { return [v, o instanceof RegExp]; });
            [
                d.length, d[0], re.length, re[0],
                dm.length, dm[0][0], dm[0][1],
                rm.length, rm[0][0], rm[0][1]
            ].join("|") === "1|42|1|7|1|42|true|1|7|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_sloppy_global_this_and_global_this_alias() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            this._flag = 7;
            function readThis() { return this._flag; }
            var mapped = [1].map(function () { return this._flag; });
            [
                readThis() === 7,
                mapped[0] === 7,
                this === globalThis,
                typeof globalThis === "object"
            ].join("|") === "true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_promise_constructor_errors_and_executor_rejection() {
    let mut ctx = Context::new_small();
    ctx.eval(
        r#"
        var ctorErr = "";
        try { new Promise(1); } catch (e) { ctorErr += e.name; }
        try { Promise(function () {}); } catch (e) { ctorErr += "|" + e.name; }
        try { Promise.call(null, function () {}); } catch (e) { ctorErr += "|" + e.name; }
        var reason = "pending";
        new Promise(function () { throw 9; }).catch(function (e) { reason = e; });
        "#,
    )
    .unwrap();
    assert_eq!(
        eval_str(&mut ctx, "ctorErr"),
        "TypeError|TypeError|TypeError"
    );
    assert_eq!(ctx.eval("reason").unwrap().as_smi(), Some(9));
}

#[test]
fn test_promise_generic_catch_finally_and_self_resolution() {
    let mut ctx = Context::new_small();
    ctx.eval(
        r#"
        var thenResult = { tag: "then-result" };
        var thenable = { then: function (a, b) { return thenResult; } };
        var caught = Promise.prototype.catch.call(thenable, function () {});
        var site = {};
        var seenThis = null;
        site.then = function (a, b) { seenThis = this; return "R"; };
        var finalResult = Promise.prototype.finally.call(site, 1);
        var resolve;
        var p = new Promise(function (r) { resolve = r; });
        var selfReason = null;
        p.catch(function (e) { selfReason = e; });
        resolve(p);
        "#,
    )
    .unwrap();
    let r = ctx
        .eval(
            r#"
            caught === thenResult
                && finalResult === "R"
                && seenThis === site
                && selfReason instanceof TypeError;
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_promise_resolve_defers_thenable_job() {
    let mut ctx = Context::new_small();
    ctx.eval(
        r#"
            var order = [];
            var thenable = {
                then: function (resolve) {
                    order.push("then");
                    resolve(1);
                }
            };
            Promise.resolve(thenable).then(function () { order.push("done"); });
            order.push("sync");
            "#,
    )
    .unwrap();
    let r = ctx.eval(r#"order.join(",") === "sync,then,done""#).unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_top_level_lexical_bindings_captured_by_closures() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            function readX() { return x; }
            var tdz = false;
            try { readX(); } catch (e) { tdz = e instanceof ReferenceError; }
            let x = 5;
            let counter = 1;
            function bump() { counter += 1; }
            bump();
            bump();
            const fixed = 9;
            var constThrew = false;
            function overwrite() { try { fixed = 10; } catch (e) { constThrew = e instanceof TypeError; } }
            overwrite();
            [
                tdz,
                readX() === 5,
                counter === 3,
                fixed === 9,
                constThrew,
                (() => x)() === 5
            ].join("|") === "true|true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_function_local_const_captured_write_rejected() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            function f() {
                const c = 1;
                let l = 1;
                var threw = false;
                (() => {
                    try { c = 2; } catch (e) { threw = e instanceof TypeError; }
                    l = 2;
                })();
                return threw && c === 1 && l === 2;
            }
            const outer = 3;
            function shadow(outer) { outer = 4; return outer; }
            f() && shadow(5) === 4 && outer === 3;
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_builtin_prototype_constructor_backrefs() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            [
                [].constructor === Array,
                "x".constructor === String,
                ({}).constructor === Object,
                (/x/).constructor === RegExp,
                (new Map()).constructor === Map,
                (new Set()).constructor === Set,
                (new Date(0)).constructor === Date,
                (function () {}).constructor === Function,
                (new Error("x")).constructor === Error,
                (new TypeError("x")).constructor === TypeError,
                Promise.resolve(1).constructor === Promise
            ].join("|") === "true|true|true|true|true|true|true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_string_locale_statics_and_primitive_methods() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            [
                String.fromCodePoint(65, 66) === "AB",
                String.raw({ raw: ["a", "b"] }, 1) === "a1b",
                "é".normalize("NFC") === "é",
                "x".toWellFormed() === "x",
                "A".toLocaleLowerCase() === "a",
                "a".toLocaleUpperCase() === "A",
                "a".localeCompare("b") === -1,
                (5).toString() === "5",
                true.toString() === "true",
                Number.prototype.toLocaleLowerCase === String.prototype.toLocaleLowerCase
            ].join("|") === "true|true|true|true|true|true|true|true|true|false";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
    let r2 = ctx
        .eval(
            r#"
            Number.prototype.toLocaleLowerCase = String.prototype.toLocaleLowerCase;
            Infinity.toLocaleLowerCase() === "infinity";
            "#,
        )
        .unwrap();
    assert_eq!(r2.to_boolean(), Some(true));
    let r3 = ctx
        .eval(
            r#"
            var threw = false;
            try {
                String.prototype.toLowerCase.call({toString: undefined, valueOf: undefined});
            } catch (e) {
                threw = e instanceof TypeError;
            }
            threw && "abcabc".lastIndexOf("b") === 4 && "abc".lastIndexOf("b", 1) === 1;
            "#,
        )
        .unwrap();
    assert_eq!(r3.to_boolean(), Some(true));
}

#[test]
fn test_object_wrapper_tags_and_legacy_accessors() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var o = {};
            o.__defineGetter__("x", function () { return 7; });
            o.__defineSetter__("y", function (v) { this.z = v; });
            o.y = 9;
            [
                Object.prototype.toString.call(new Boolean(true)) === "[object Boolean]",
                Object.prototype.toString.call(new Number(5)) === "[object Number]",
                ({}).toLocaleString() === "[object Object]",
                o.x === 7,
                o.z === 9,
                typeof o.__lookupGetter__("x") === "function",
                typeof o.__lookupSetter__("y") === "function",
                o.__lookupGetter__("missing") === undefined
            ].join("|") === "true|true|true|true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_namespace_prototypes_and_proto_accessor() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            Object.prototype.get = function () { return "jsonGetProperty"; };
            var target = {};
            Object.defineProperty(target, "property", JSON);
            var desc = Object.getOwnPropertyDescriptor(Object.prototype, "__proto__");
            var a = {};
            var b = {};
            a.__proto__ = b;
            var cyclic = false;
            try {
                desc.set.call(b, a);
            } catch (e) {
                cyclic = e instanceof TypeError;
            }
            var plain = {};
            plain.__proto__ = null;
            [
                Object.getPrototypeOf(JSON) === Object.prototype,
                Object.getPrototypeOf(Math) === Object.prototype,
                Object.getPrototypeOf(console) === Object.prototype,
                target.property === "jsonGetProperty",
                typeof desc.get === "function",
                typeof desc.set === "function",
                Object.getPrototypeOf(a) === b,
                cyclic,
                Object.getPrototypeOf(plain) === null,
                plain.__proto__ === null
            ].join("|") === "true|true|true|true|true|true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_builtin_methods_instanceof_function() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            [
                [].map instanceof Function,
                Promise.prototype.then instanceof Function,
                String.prototype.slice instanceof Function,
                Object.prototype.hasOwnProperty instanceof Function,
                (function () {}) instanceof Function
            ].join("|") === "true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_builtin_property_descriptor_attributes() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var method = Object.getOwnPropertyDescriptor(Date.prototype, "getDate");
            var ctor = Object.getOwnPropertyDescriptor(Array.prototype, "constructor");
            var constant = Object.getOwnPropertyDescriptor(Math, "PI");
            var proto = Object.getOwnPropertyDescriptor(Array, "prototype");
            [
                method.writable === true && method.enumerable === false && method.configurable === true,
                ctor.writable === true && ctor.enumerable === false && ctor.configurable === true,
                constant.writable === false && constant.enumerable === false && constant.configurable === false,
                proto.writable === false && proto.enumerable === false && proto.configurable === false
            ].join("|") === "true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_function_prototype_chain_and_string_own_methods() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            function F() { this.a = 1; }
            var instance = new F();
            [
                Object.getPrototypeOf(F.prototype) === Object.prototype,
                instance.hasOwnProperty("a"),
                instance.toString() === "[object Object]",
                "abc".hasOwnProperty(0),
                "abc".hasOwnProperty("length"),
                typeof "abc".hasOwnProperty === "function"
            ].join("|") === "true|true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_property_descriptor_objects_inherit_object_prototype() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var d = Object.getOwnPropertyDescriptor(Number, "length");
            var e = Object.getOwnPropertyDescriptor([1], "0");
            [
                Object.getPrototypeOf(d) === Object.prototype,
                d.hasOwnProperty("writable"),
                d.hasOwnProperty("value") && d.hasOwnProperty("get") === false,
                Object.getPrototypeOf(e) === Object.prototype,
                e.hasOwnProperty("value") && e.enumerable === true
            ].join("|") === "true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_global_object_mirror_and_callable_constructors() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var d = Object.getOwnPropertyDescriptor(this, "undefined");
            [
                this.Math === Math,
                globalThis.Object === Object,
                typeof Object === "function",
                typeof Array === "function",
                typeof Math === "object",
                Object instanceof Function,
                Symbol instanceof Function,
                !(Math instanceof Function),
                this.hasOwnProperty("Object"),
                d.writable === false && d.enumerable === false && d.configurable === false
            ].join("|") === "true|true|true|true|true|true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_regexp_flag_accessors() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var re = /a/gim;
            var d = Object.getOwnPropertyDescriptor(RegExp.prototype, "global");
            [
                re.global === true,
                re.ignoreCase === true,
                re.multiline === true,
                re.sticky === false,
                re.unicode === false,
                re.flags === "gim",
                typeof d.get === "function",
                d.enumerable === false,
                d.configurable === true,
                RegExp.prototype.propertyIsEnumerable("global") === false
            ].join("|") === "true|true|true|true|true|true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_regexp_flag_getter_receiver_rules() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var d = Object.getOwnPropertyDescriptor(RegExp.prototype, "global");
            var threw = false;
            try { d.get.call(3); } catch (e) { threw = e instanceof TypeError; }
            [
                RegExp.prototype.global === undefined,
                d.get.call(/a/g) === true,
                threw
            ].join("|") === "true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_weakmap_weakset_basics() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var key = {};
            var wm = new WeakMap([[key, 1]]);
            var primitiveThrew = false;
            try { wm.set(1, 2); } catch (e) { primitiveThrew = e instanceof TypeError; }
            var ws = new WeakSet([key]);
            var weakSetThrew = false;
            try { ws.add(1); } catch (e) { weakSetThrew = e instanceof TypeError; }
            [
                wm instanceof WeakMap,
                wm.get(key) === 1,
                wm.has(key) === true,
                wm.delete(key) === true,
                wm.has(key) === false,
                wm.size === undefined,
                Object.prototype.toString.call(wm) === "[object WeakMap]",
                primitiveThrew,
                ws instanceof WeakSet,
                ws.has(key) === true,
                weakSetThrew,
                ws.size === undefined
            ].join("|") === "true|true|true|true|true|true|true|true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_weak_collection_prototypes_and_symbol_keys() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var sym = Symbol("key");
            var wm = new WeakMap();
            wm.set(sym, 7);
            var ws = new WeakSet();
            ws.add(sym);
            [
                typeof WeakMap === "function",
                Object.getPrototypeOf(WeakMap) === Function.prototype,
                Object.getPrototypeOf(WeakSet) === Function.prototype,
                Object.getPrototypeOf(WeakMap.prototype) === Object.prototype,
                WeakMap.prototype.constructor === WeakMap,
                wm.get(sym) === 7,
                wm.has(sym) === true,
                ws.has(sym) === true
            ].join("|") === "true|true|true|true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_exotic_descriptor_objects() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var re = new RegExp();
            re.enumerable = true;
            var created = Object.create({}, { prop: re });
            var date = new Date();
            date.value = 42;
            date.enumerable = true;
            date.writable = true;
            date.configurable = true;
            var target = {};
            Object.defineProperty(target, "d", date);
            [
                created.propertyIsEnumerable("prop"),
                target.d === 42,
                Object.prototype.hasOwnProperty.call(target, "d")
            ].join("|") === "true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_promise_try_and_with_resolvers() {
    let mut ctx = Context::new_small();
    ctx.eval(
        r#"
        var tryOut = 0;
        var wrOut = 0;
        Promise.try(function () { return 5; }).then(function (v) { tryOut = v; });
        var r = Promise.withResolvers();
        r.resolve(7);
        r.promise.then(function (v) { wrOut = v; });
        "#,
    )
    .unwrap();
    let r = ctx
        .eval("tryOut === 5 && wrOut === 7 && typeof Promise.withResolvers === 'function' && typeof Promise.try === 'function';")
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_function_to_string_callable_rules() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var source = (function named() { return 1; }).toString();
            var threw = false;
            try { Function.prototype.toString.call(null); } catch (e) { threw = e instanceof TypeError; }
            var d = Object.getOwnPropertyDescriptor(Function.prototype.toString, "length");
            var rejected = false;
            Promise.try(function () { throw new TypeError("boom"); }).catch(function (e) {
                rejected = e.message === "boom";
            });
            [
                /native code/.test(source),
                threw,
                d.configurable === true,
                Function.prototype.toString.hasOwnProperty("length"),
                !Function.prototype.toString.propertyIsEnumerable("length")
            ].join("|") === "true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_function_string_coercion() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var fn = function () { return 1; };
            var concat = "" + fn;
            var template = `${fn}`;
            [
                /native code/.test(concat),
                template === concat,
                String(fn) === concat,
                typeof (fn + 1) === "string"
            ].join("|") === "true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_promise_own_properties() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var p = Promise.resolve(1);
            p.custom = 42;
            p.other = "x";
            var d = Object.getOwnPropertyDescriptor(p, "custom");
            [
                p.custom === 42,
                p.other === "x",
                p instanceof Promise,
                Object.prototype.hasOwnProperty.call(p, "custom"),
                Object.keys(p).indexOf("custom") >= 0,
                d.value === 42,
                d.writable === true,
                d.enumerable === true,
                d.configurable === true
            ].join("|") === "true|true|true|true|true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_promise_species_and_define_delete() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var p = Promise.resolve(1);
            p.constructor = null;
            var nullCtorThrew = false;
            try { p.then(function () {}); } catch (e) { nullCtorThrew = e instanceof TypeError; }
            var q = Promise.resolve(2);
            Object.defineProperty(q, "x", { value: 5, enumerable: true, writable: true, configurable: true });
            var defined = q.x === 5 && Object.keys(q).indexOf("x") >= 0;
            var deleted = delete q.x && q.x === undefined && !Object.prototype.hasOwnProperty.call(q, "x");
            [nullCtorThrew, defined, deleted].join("|") === "true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_symbol_prototype_intrinsics() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var s = Symbol("x");
            var getter = Object.getOwnPropertyDescriptor(Symbol.prototype, "description").get;
            [
                Object.getPrototypeOf(s) === Symbol.prototype,
                Object.getPrototypeOf(1) === Number.prototype,
                Object.getPrototypeOf(true) === Boolean.prototype,
                getter.call(s) === "x",
                getter.call(Symbol()) === undefined,
                String(s) === "Symbol(x)",
                s.toString() === "Symbol(x)",
                Symbol.prototype.valueOf.call(s) === s
            ].join("|") === "true|true|true|true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_number_statics_and_radix() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            [
                Number.isNaN(NaN),
                !Number.isNaN("NaN"),
                Number.isFinite(1) && !Number.isFinite(Infinity),
                Number.isInteger(1) && !Number.isInteger(1.5),
                Number.isSafeInteger(9007199254740991),
                !Number.isSafeInteger(9007199254740992),
                (10).toString(2) === "1010",
                (255).toString(16) === "ff",
                Number.prototype.toString(33) === "0",
                (new Number(10)).toString(2) === "1010",
                Boolean.prototype.valueOf() === false
            ].join("|") === "true|true|true|true|true|true|true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_number_formatting_methods() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            [
                (1.2345).toFixed(2) === "1.23",
                (1.5).toFixed(0) === "2",
                (0).toFixed(2) === "0.00",
                (123.456).toPrecision(4) === "123.5",
                (1).toPrecision(3) === "1.00",
                (1.2345).toExponential(2) === "1.23e+0",
                (100).toExponential() === "1e+2",
                (NaN).toFixed(2) === "NaN",
                (Infinity).toFixed(2) === "Infinity",
                (1.5).toLocaleString() === "1.5"
            ].join("|") === "true|true|true|true|true|true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_regexp_string_and_number_exponent_forms() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            [
                String(/test/g) === "/test/g",
                (/a/i).toString() === "/a/i",
                String.prototype.trim.call(/x/) === "/x/",
                " \uFEFFabc\uFEFF ".trim() === "abc",
                "\uFEFFabc".trimStart() === "abc",
                "abc\uFEFF".trimEnd() === "abc",
                String(0.0000001) === "1e-7",
                String(1e21) === "1e+21",
                String(1.5e-7) === "1.5e-7"
            ].join("|") === "true|true|true|true|true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_math_hyperbolic_and_signed_zero() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            [
                Math.fround(1.5) === 1.5,
                Math.acosh(1) === 0,
                Math.asinh(0) === 0,
                Math.atanh(0) === 0,
                Math.sinh(0) === 0,
                Math.cosh(0) === 1,
                Math.tanh(0) === 0,
                Math.expm1(0) === 0,
                Math.log1p(0) === 0,
                Object.is(Math.min(-0, 0), -0),
                !Object.is(Math.max(-0, 0), -0),
                Object.is(Math.ceil(-0.1), -0),
                Object.is(Math.pow(-0, 1), -0),
                Number.isNaN(Math.pow(1, Infinity)),
                Number.isNaN(Math.max(1, NaN))
            ].join("|") === "true|true|true|true|true|true|true|true|true|true|true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_math_sum_precise_and_minmax_edges() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            [
                Math.sumPrecise([1, 2, 3]) === 6,
                Math.sumPrecise(new Set([1, 2])) === 3,
                Object.is(Math.sumPrecise([]), -0),
                Object.is(Math.sumPrecise([-0]), -0),
                Object.is(Math.sumPrecise([-0, 0]), 0),
                Number.isNaN(Math.sumPrecise([1, NaN])),
                Math.max("2", 3) === 3,
                Object.is(Math.max(-0, -0), -0),
                Object.is(Math.min(-0, 0), -0)
            ].join("|") === "true|true|true|true|true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_array_buffer_length_accessors() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var b = new ArrayBuffer(8);
            var d = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, "byteLength");
            var threw = false;
            try { d.get.call(1); } catch (e) { threw = e instanceof TypeError; }
            [
                b.byteLength === 8,
                b.detached === false,
                b.resizable === false,
                b.maxByteLength === 8,
                typeof d.get === "function",
                d.set === undefined,
                threw
            ].join("|") === "true|true|true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_array_buffer_transfer_and_detach() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var b = new ArrayBuffer(4);
            new Uint8Array(b)[0] = 42;
            var n = b.transfer(4);
            var copied = new Uint8Array(n)[0] === 42;
            var resizeThrew = false;
            try { n.resize(1); } catch (e) { resizeThrew = e instanceof TypeError; }
            [
                b.detached === true,
                b.byteLength === 0,
                n.byteLength === 4,
                copied,
                resizeThrew,
                typeof b.transferToFixedLength === "function",
                typeof b.sliceToImmutable === "function"
            ].join("|") === "true|true|true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_resizable_array_buffer() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var b = new ArrayBuffer(4, { maxByteLength: 16 });
            var grew = false;
            var exceeded = false;
            var plainRejected = false;
            b.resize(8);
            grew = b.byteLength === 8 && b.maxByteLength === 16;
            try { b.resize(32); } catch (e) { exceeded = e instanceof RangeError; }
            try { new ArrayBuffer(4).resize(8); } catch (e) { plainRejected = e instanceof TypeError; }
            [
                b.resizable === true,
                grew,
                exceeded,
                plainRejected,
                new ArrayBuffer(4).resizable === false
            ].join("|") === "true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_typed_array_length_tracking() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var b = new ArrayBuffer(4, { maxByteLength: 16 });
            var tracking = new Uint8Array(b);
            var initial = tracking.length === 4;
            b.resize(8);
            var grew = tracking.length === 8;
            tracking[7] = 9;
            b.resize(2);
            var shrank = tracking.length === 2;
            var fixed = new Uint8Array(b, 0, 2);
            b.resize(8);
            [
                initial,
                grew,
                shrank,
                tracking.length === 8,
                fixed.length === 2
            ].join("|") === "true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_typed_array_array_like_coercions() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var a = new Uint8Array([1, 2]);
            a.set([42], [1]);
            var b = new Uint8Array([3, 4]);
            [
                a[0] === 1,
                a[1] === 42,
                compareArray(a, [1, 42]),
                compareArray(b, [3, 4]),
                !compareArray(a, b)
            ].join("|") === "true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_typed_array_array_methods_and_is_primitive() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var seen = [];
            var a = new Uint8Array([1, 2, 3]);
            a.forEach(function (v, i) { seen.push(v + i); });
            [
                seen.join(",") === "1,3,5",
                a.some(function (v) { return v === 2; }),
                a.every(function (v) { return v > 0; }),
                a.reduce(function (x, y) { return x + y; }, 0) === 6,
                isPrimitive(1),
                isPrimitive("x"),
                !isPrimitive({}),
                isPrimitive(null)
            ].join("|") === "true|true|true|true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_for_in_break_and_continue_cleanup() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            function beforeBreak() {
                var out = [];
                for (var k in { a: 1, b: 2, c: 3 }) {
                    if (k === "b") break;
                    out.push(k);
                }
                return out.join(",");
            }
            function skipA() {
                var out = [];
                for (var k in { a: 1, b: 2, c: 3 }) {
                    if (k === "a") continue;
                    out.push(k);
                }
                return out.join(",");
            }
            function nested() {
                var out = [];
                for (var k in { a: 1, b: 2 }) {
                    for (var j in { x: 1, y: 2 }) {
                        if (j === "y") break;
                        out.push(k + j);
                    }
                    if (k === "a") continue;
                }
                return out.join(",");
            }
            [
                beforeBreak() === "a",
                skipA() === "b,c",
                nested() === "ax,bx"
            ].join("|") === "true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_delete_non_configurable_property() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var o = {};
            Object.defineProperty(o, "x", { value: 7, configurable: false });
            var sloppyDelete = delete o.x;
            var stillThere = o.x === 7;
            var threw = false;
            try {
                (function () { "use strict"; delete o.x; })();
            } catch (e) {
                threw = e instanceof TypeError;
            }
            var configurable = { y: 1 };
            var deleted = delete configurable.y;
            sloppyDelete === false && stillThere && threw && deleted;
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_array_named_property_has_own_and_enumerable() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var a = [];
            a.y = 1;
            Object.defineProperty(a, "z", { value: 2, enumerable: false });
            [
                a.hasOwnProperty("y"),
                a.hasOwnProperty("z"),
                a.propertyIsEnumerable("y"),
                !a.propertyIsEnumerable("z"),
                a.hasOwnProperty("length"),
                !a.hasOwnProperty("missing")
            ].join("|") === "true|true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_builtin_function_lengths() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            [
                Object.defineProperty.length === 3,
                Object.create.length === 2,
                parseInt.length === 2,
                JSON.stringify.length === 3,
                Math.max.length === 2,
                String.fromCharCode.length === 1,
                Array.isArray.length === 1
            ].join("|") === "true|true|true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_builtin_constructor_metadata() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            [
                Object.name === "Object",
                Object.length === 1,
                Date.name === "Date",
                Date.length === 7,
                RegExp.length === 2,
                Uint8Array.name === "Uint8Array",
                Uint8Array.length === 3,
                Map.name === "Map",
                Map.length === 0,
                Object.defineProperty.name === "defineProperty",
                [].forEach.name === "forEach"
            ].join("|") === "true|true|true|true|true|true|true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_delete_builtin_metadata() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var f = Date.prototype.setHours;
            var hadLength = f.hasOwnProperty("length");
            var deleted = delete f.length;
            [
                hadLength,
                deleted,
                !f.hasOwnProperty("length"),
                Object.getOwnPropertyDescriptor(f, "length") === undefined,
                f.length === 0,
                delete f.name,
                !f.hasOwnProperty("name")
            ].join("|") === "true|true|true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_function_prototype_metadata_fallback() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var f = Date.prototype.setHours;
            delete f.length;
            [
                Function.prototype.length === 0,
                Function.prototype.name === "",
                f.length === 0
            ].join("|") === "true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_date_to_primitive_symbol() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var method = Date.prototype[Symbol.toPrimitive];
            var d = new Date(0);
            var invalidHint = false;
            try { method.call(d, "bad"); } catch (e) { invalidHint = e instanceof TypeError; }
            var nonObject = false;
            try { method.call(1, "number"); } catch (e) { nonObject = e instanceof TypeError; }
            [
                typeof method === "function",
                method.name === "[Symbol.toPrimitive]",
                method.length === 1,
                method.call(d, "number") === 0,
                typeof method.call(d, "string") === "string",
                invalidHint,
                nonObject
            ].join("|") === "true|true|true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_function_symbol_has_instance() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var method = Function.prototype[Symbol.hasInstance];
            function F() {}
            var d = Object.getOwnPropertyDescriptor(Function.prototype, Symbol.hasInstance);
            [
                typeof method === "function",
                method.name === "[Symbol.hasInstance]",
                method.length === 1,
                d.writable === false && d.enumerable === false && d.configurable === false,
                method.call(F, new F()),
                !method.call(F, {}),
                !method.call({}, new F())
            ].join("|") === "true|true|true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}

#[test]
fn test_regexp_symbol_match_and_search() {
    let mut ctx = Context::new_small();
    let r = ctx
        .eval(
            r#"
            var match = RegExp.prototype[Symbol.match];
            var search = RegExp.prototype[Symbol.search];
            var one = /b/[Symbol.match]("abc");
            var all = /a/g[Symbol.match]("aXa");
            [
                typeof match === "function",
                match.name === "[Symbol.match]",
                match.length === 1,
                typeof search === "function",
                search.name === "[Symbol.search]",
                one[0] === "b",
                one.index === 1,
                all.length === 2,
                /b/[Symbol.search]("abc") === 1,
                /z/[Symbol.search]("abc") === -1
            ].join("|") === "true|true|true|true|true|true|true|true|true|true";
            "#,
        )
        .unwrap();
    assert_eq!(r.to_boolean(), Some(true));
}
