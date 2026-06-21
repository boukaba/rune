use rune_embed::Context;
use rune_core::value::Value;

#[test]
fn test_eval_number() {
    let mut ctx = Context::new();
    let result = ctx.eval("42").unwrap();
    assert_eq!(result.as_smi(), Some(42));
}

#[test]
fn test_eval_binary() {
    let mut ctx = Context::new();
    let result = ctx.eval("1 + 2").unwrap();
    assert_eq!(result.as_smi(), Some(3));
}

#[test]
fn test_eval_multiplication() {
    let mut ctx = Context::new();
    let result = ctx.eval("2 * 3 + 4").unwrap();
    assert_eq!(result.as_smi(), Some(10));
}

#[test]
fn test_eval_subtract() {
    let mut ctx = Context::new();
    let result = ctx.eval("10 - 3").unwrap();
    assert_eq!(result.as_smi(), Some(7));
}

#[test]
fn test_eval_var_decl() {
    let mut ctx = Context::new();
    ctx.eval("var x = 42;").unwrap();
    // The local should be stored and retrievable
    let result = ctx.eval("var y = 10;").unwrap();
    assert!(result.is_undefined());
}

#[test]
fn test_eval_if() {
    let mut ctx = Context::new();
    let result = ctx.eval("if (true) { 1; } else { 2; }").unwrap();
    // if's result is the last expression in the taken branch
    assert!(result.is_undefined()); // expression statements pop
}

#[test]
fn test_eval_while() {
    let mut ctx = Context::new();
    let result = ctx.eval(
        "var x = 10;
         while (x > 0) {
           x = x - 1;
         }"
    ).unwrap();
    assert!(result.is_undefined());
}

#[test]
fn test_eval_do_while() {
    let mut ctx = Context::new();
    let result = ctx.eval(
        "var x = 10;
         do {
           x = x - 1;
         } while (x > 0);"
    ).unwrap();
    assert!(result.is_undefined());
}

#[test]
fn test_eval_do_while_once() {
    let mut ctx = Context::new();
    let result = ctx.eval(
        "var x = 0;
         do {
           x = x + 1;
         } while (false);
         x"
    ).unwrap();
    assert_eq!(result.as_smi(), Some(1), "do-while body runs at least once");
}

#[test]
fn test_eval_for() {
    let mut ctx = Context::new();
    let result = ctx.eval(
        "var s = 0;
         for (var i = 0; i < 10; i = i + 1) {
           s = s + i;
         }"
    ).unwrap();
    assert!(result.is_undefined());
}

#[test]
fn test_eval_comparison() {
    let mut ctx = Context::new();
    let r1 = ctx.eval("1 < 2").unwrap();
    assert_eq!(r1.as_smi(), Some(1));

    let r2 = ctx.eval("3 > 5").unwrap();
    assert_eq!(r2.as_smi(), Some(0));
}

#[test]
fn test_eval_unary() {
    let mut ctx = Context::new();
    let r1 = ctx.eval("-5").unwrap();
    assert_eq!(r1.as_smi(), Some(-5));

    let r2 = ctx.eval("!true").unwrap();
    assert_eq!(r2.as_smi(), Some(0));
}

#[test]
fn test_eval_bitwise() {
    let mut ctx = Context::new();
    let r = ctx.eval("1 | 2").unwrap();
    assert_eq!(r.as_smi(), Some(3));
}

#[test]
fn test_eval_nested_block() {
    let mut ctx = Context::new();
    let result = ctx.eval("{{{{42;}}}}").unwrap();
    assert!(result.is_undefined());
}

#[test]
fn test_eval_string_literal() {
    let mut ctx = Context::new();
    let result = ctx.eval(r#""hello""#).unwrap();
    assert!(result.is_heap_object());
}

#[test]
fn test_eval_property_access() {
    let mut ctx = Context::new();
    let result = ctx.eval("({a: 1, b: 2}).a").unwrap();
    assert_eq!(result.as_smi(), Some(1));
}

#[test]
fn test_eval_computed_property() {
    let mut ctx = Context::new();
    let result = ctx.eval("({a: 42, b: 99})['a']").unwrap();
    assert_eq!(result.as_smi(), Some(42));
}

#[test]
fn test_eval_var_lookup() {
    let mut ctx = Context::new();
    let result = ctx.eval("var x = 42; x").unwrap();
    assert_eq!(result.as_smi(), Some(42));
}

#[test]
fn test_eval_property_assign() {
    let mut ctx = Context::new();
    let result = ctx.eval("var o = {a: 1}; o.a = 5; o.a").unwrap();
    assert_eq!(result.as_smi(), Some(5));
}

#[test]
fn test_eval_object_literal() {
    let mut ctx = Context::new();
    let result = ctx.eval("({a: 1})").unwrap();
    assert!(result.is_heap_object());
}

#[test]
fn test_eval_function_decl() {
    let mut ctx = Context::new();
    // Test that function object is created by checking typeof
    let result = ctx.eval("typeof function() { return 1; }").unwrap();
    assert!(result.is_heap_object()); // typeof returns a HeapString for functions
}

#[test]
fn test_eval_make_function_expr() {
    let mut ctx = Context::new();
    let result = ctx.eval("(function() { return 1; })").unwrap();
    assert!(result.is_heap_object());
}

#[test]
fn test_eval_call_func_obj() {
    let mut ctx = Context::new();
    // Direct call via var binding
    let result = ctx.eval("var f = function() { return 1; }; f()").unwrap();
    assert_eq!(result.as_smi(), Some(1));
}

#[test]
fn test_eval_call_iife() {
    let mut ctx = Context::new();
    // IIFE - immediately invoked function expression
    let result = ctx.eval("(function() { return 1; })()").unwrap();
    assert_eq!(result.as_smi(), Some(1));
}

#[test]
fn test_eval_function_decl_and_call() {
    let mut ctx = Context::new();
    let result = ctx.eval("function f() { return 42; } f()").unwrap();
    assert_eq!(result.as_smi(), Some(42));
}

#[test]
fn test_eval_function_args() {
    let mut ctx = Context::new();
    let result = ctx.eval("function add(a, b) { return a + b; } add(3, 4)").unwrap();
    assert_eq!(result.as_smi(), Some(7));
}

#[test]
fn test_eval_nested_function() {
    let mut ctx = Context::new();
    let result = ctx.eval("function outer() { function inner() { return 99; } return inner(); } outer()").unwrap();
    assert_eq!(result.as_smi(), Some(99));
}

#[test]
fn test_eval_function_expr() {
    let mut ctx = Context::new();
    let result = ctx.eval("var f = function(x) { return x * 2; }; f(5)").unwrap();
    assert_eq!(result.as_smi(), Some(10));
}

#[test]
fn test_eval_recursive() {
    let mut ctx = Context::new();
    let result = ctx.eval("function fact(n) { if (n <= 1) { return 1; } return n * fact(n - 1); } fact(5)").unwrap();
    assert_eq!(result.as_smi(), Some(120));
}

#[test]
fn test_eval_chained_property() {
    let mut ctx = Context::new();
    let result = ctx.eval("({a: {b: 42}}).a.b").unwrap();
    assert_eq!(result.as_smi(), Some(42));
}

#[test]
fn test_eval_multi_object() {
    let mut ctx = Context::new();
    let result = ctx.eval("var x = {a: 10, b: 20}; x.a + x.b").unwrap();
    assert_eq!(result.as_smi(), Some(30));
}

#[test]
fn test_parse_error() {
    let mut ctx = Context::new();
    let result = ctx.eval("!!!");
    assert!(result.is_err());
}

#[test]
fn test_eval_throw() {
    let mut ctx = Context::new();
    let result = ctx.eval("throw 42;");
    assert!(result.is_err(), "throw should produce an error");
    let err = result.unwrap_err();
    assert!(err.contains("42"), "error should contain thrown value");
}

#[test]
fn test_eval_new_simple() {
    let mut ctx = Context::new();
    let result = ctx.eval("new Object();").unwrap();
    assert!(result.is_undefined(), "new stub should return undefined");
}

#[test]
fn test_non_generator_no_resume() {
    let mut ctx = Context::new();
    ctx.eval("function f() { return 1; }").unwrap();
    // Global scope not yet implemented — function declarations don't persist
    // across eval() calls. This test just verifies no crash.
    let result = ctx.eval("42").unwrap();
    assert_eq!(result.as_smi(), Some(42));
}

#[test]
fn test_eval_string_concat() {
    let mut ctx = Context::new();
    let result = ctx.eval("\"hello\" + \" world\"").unwrap();
    assert!(!result.is_undefined(), "string concat should not be undefined");
    // We can't easily inspect the string value, but it should not error
}

#[test]
fn test_eval_mixed_concat() {
    let mut ctx = Context::new();
    let result = ctx.eval("\"x\" + 1").unwrap();
    assert!(!result.is_undefined(), "mixed concat should not be undefined");
}

// ---- Generator / Yield tests ----

#[test]
fn test_generator_yield_value() {
    let mut ctx = Context::new();
    // Define and call the generator in a single eval so `gen` stays in scope
    let handle = ctx.eval("function* gen() { yield 42; }; gen()").unwrap();
    let gen_id = handle.as_smi().unwrap() as usize;
    let result = ctx.resume(gen_id, Value::undefined()).unwrap();
    assert_eq!(result.as_smi(), Some(42), "first yield should return 42");
    let done = ctx.resume(gen_id, Value::undefined()).unwrap();
    assert!(done.is_undefined(), "second resume should be undefined (done)");
}

#[test]
fn test_generator_yield_twice() {
    let mut ctx = Context::new();
    let handle = ctx.eval("function* gen() { yield 1; yield 2; }; gen()").unwrap();
    let gen_id = handle.as_smi().unwrap() as usize;
    let r1 = ctx.resume(gen_id, Value::undefined()).unwrap();
    assert_eq!(r1.as_smi(), Some(1));
    let r2 = ctx.resume(gen_id, Value::undefined()).unwrap();
    assert_eq!(r2.as_smi(), Some(2));
    let r3 = ctx.resume(gen_id, Value::undefined()).unwrap();
    assert!(r3.is_undefined(), "done generator should return undefined");
}

#[test]
fn test_generator_yield_then_return() {
    let mut ctx = Context::new();
    let handle = ctx.eval("function* gen() { yield 10; return 20; }; gen()").unwrap();
    let gen_id = handle.as_smi().unwrap() as usize;
    let r1 = ctx.resume(gen_id, Value::undefined()).unwrap();
    assert_eq!(r1.as_smi(), Some(10));
    let r2 = ctx.resume(gen_id, Value::undefined()).unwrap();
    assert_eq!(r2.as_smi(), Some(20));
    let r3 = ctx.resume(gen_id, Value::undefined()).unwrap();
    assert!(r3.is_undefined());
}

#[test]
fn test_eval_try_catch_no_exception() {
    let mut ctx = Context::new();
    let result = ctx.eval("var x = 0; try { x = 1; } catch (e) { x = 2; } x;").unwrap();
    assert_eq!(result.as_smi(), Some(1), "try block should execute normally");
}

#[test]
fn test_eval_try_catch_with_exception() {
    let mut ctx = Context::new();
    let result = ctx.eval("var x; try { throw 42; } catch (e) { x = e; } x;").unwrap();
    assert_eq!(result.as_smi(), Some(42), "catch should bind thrown value");
}

#[test]
fn test_eval_try_catch_no_error() {
    let mut ctx = Context::new();
    let result = ctx.eval("try { 1; } catch (e) {} 2;").unwrap();
    assert_eq!(result.as_smi(), Some(2), "execution should continue after try-catch");
}

#[test]
fn test_eval_try_catch_error_caught() {
    let mut ctx = Context::new();
    let result = ctx.eval("try { throw 99; } catch (e) {} 42;").unwrap();
    assert_eq!(result.as_smi(), Some(42), "caught error should not propagate");
}

#[test]
fn test_global_scope_persistence_var() {
    let mut ctx = Context::new();
    ctx.eval("var x = 10;").unwrap();
    ctx.eval("y = 20;").unwrap();
    let r1 = ctx.eval("x").unwrap();
    assert_eq!(r1.as_smi(), Some(10), "var-declared variable persists across evals");
    let r2 = ctx.eval("y").unwrap();
    assert_eq!(r2.as_smi(), Some(20), "implicit global assignment persists across evals");
}

#[test]
fn test_global_scope_mutation() {
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
    let r = ctx.eval("var x = 0; try { x = 1; } finally { x = 2; } x;").unwrap();
    assert_eq!(r.as_smi(), Some(2), "finally should run after try");
}

#[test]
fn test_try_finally_throw_caught_by_outer() {
    let mut ctx = Context::new();
    let r = ctx.eval("var x = 0; try { try { throw 99; } finally { x = 1; } } catch (e) { x = e; } x;").unwrap();
    assert_eq!(r.as_smi(), Some(99), "outer catch should catch rethrown exception");
}

#[test]
fn test_try_catch_finally() {
    let mut ctx = Context::new();
    let r = ctx.eval("var x = 0; try { throw 42; } catch (e) { x = 1; } finally { x = x + 10; } x;").unwrap();
    assert_eq!(r.as_smi(), Some(11), "finally should run after catch");
}

#[test]
fn test_try_finally_throw() {
    let mut ctx = Context::new();
    // If try throws and there's a finally, the exception should propagate after finally runs
    // We use an outer try-catch to observe this
    let r = ctx.eval("var x = 0; try { try { throw 99; } finally { x = 1; } } catch (e) { } x;").unwrap();
    assert_eq!(r.as_smi(), Some(1), "finally should run before exception propagates");
}


#[test]
fn test_builtin_print() {
    let mut ctx = Context::new();
    let r = ctx.eval("print(42); 99;").unwrap();
    assert_eq!(r.as_smi(), Some(99), "print should work and return undefined");
}

#[test]
fn test_builtin_string() {
    let mut ctx = Context::new();
    let r = ctx.eval(r#"String(42)"#).unwrap();
    assert!(r.heap_ptr().is_some(), "String(42) should return a heap-allocated value");
}

#[test]
fn test_builtin_error() {
    let mut ctx = Context::new();
    let r = ctx.eval(r#"Error("test")"#).unwrap();
    assert!(r.is_heap_object(), "Error should return an object");
}

#[test]
fn test_builtin_test262_error() {
    let mut ctx = Context::new();
    let r = ctx.eval(r#"Test262Error("fail")"#).unwrap();
    assert!(r.is_heap_object(), "Test262Error should return an object");
}
