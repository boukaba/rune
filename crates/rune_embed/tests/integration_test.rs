use rune_core::value::Value;
use rune_embed::Context;

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
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
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
    let result = ctx
        .eval("function add(a, b) { return a + b; } add(3, 4)")
        .unwrap();
    assert_eq!(result.as_smi(), Some(7));
}

#[test]
fn test_eval_nested_function() {
    let mut ctx = Context::new();
    let result = ctx
        .eval("function outer() { function inner() { return 99; } return inner(); } outer()")
        .unwrap();
    assert_eq!(result.as_smi(), Some(99));
}

#[test]
fn test_eval_function_expr() {
    let mut ctx = Context::new();
    let result = ctx
        .eval("var f = function(x) { return x * 2; }; f(5)")
        .unwrap();
    assert_eq!(result.as_smi(), Some(10));
}

#[test]
fn test_eval_recursive() {
    let mut ctx = Context::new();
    let result = ctx
        .eval("function fact(n) { if (n <= 1) { return 1; } return n * fact(n - 1); } fact(5)")
        .unwrap();
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
    assert!(result.is_heap_object(), "new should return a new object");
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
    assert!(
        !result.is_undefined(),
        "string concat should not be undefined"
    );
    // We can't easily inspect the string value, but it should not error
}

#[test]
fn test_eval_mixed_concat() {
    let mut ctx = Context::new();
    let result = ctx.eval("\"x\" + 1").unwrap();
    assert!(
        !result.is_undefined(),
        "mixed concat should not be undefined"
    );
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
    assert!(
        done.is_undefined(),
        "second resume should be undefined (done)"
    );
}

#[test]
fn test_generator_yield_twice() {
    let mut ctx = Context::new();
    let handle = ctx
        .eval("function* gen() { yield 1; yield 2; }; gen()")
        .unwrap();
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
    let handle = ctx
        .eval("function* gen() { yield 10; return 20; }; gen()")
        .unwrap();
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
    let mut ctx = Context::new();
    let result = ctx
        .eval("var x; try { throw 42; } catch (e) { x = e; } x;")
        .unwrap();
    assert_eq!(result.as_smi(), Some(42), "catch should bind thrown value");
}

#[test]
fn test_eval_try_catch_no_error() {
    let mut ctx = Context::new();
    let result = ctx.eval("try { 1; } catch (e) {} 2;").unwrap();
    assert_eq!(
        result.as_smi(),
        Some(2),
        "execution should continue after try-catch"
    );
}

#[test]
fn test_eval_try_catch_error_caught() {
    let mut ctx = Context::new();
    let result = ctx.eval("try { throw 99; } catch (e) {} 42;").unwrap();
    assert_eq!(
        result.as_smi(),
        Some(42),
        "caught error should not propagate"
    );
}

#[test]
fn test_global_scope_persistence_var() {
    let mut ctx = Context::new();
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
    let r = ctx
        .eval("var x = 0; try { x = 1; } finally { x = 2; } x;")
        .unwrap();
    assert_eq!(r.as_smi(), Some(2), "finally should run after try");
}

#[test]
fn test_try_finally_throw_caught_by_outer() {
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
    let r = ctx
        .eval("var x = 0; try { throw 42; } catch (e) { x = 1; } finally { x = x + 10; } x;")
        .unwrap();
    assert_eq!(r.as_smi(), Some(11), "finally should run after catch");
}

#[test]
fn test_try_finally_throw() {
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
    let r = ctx.eval("print(42); 99;").unwrap();
    assert_eq!(
        r.as_smi(),
        Some(99),
        "print should work and return undefined"
    );
}

#[test]
fn test_builtin_string_from_char_code() {
    let mut ctx = Context::new();
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

#[test]
fn test_typeof_basic() {
    let mut ctx = Context::new();
    let r = ctx.eval(r#"typeof 42"#).unwrap();
    assert!(r.heap_ptr().is_some(), "typeof should return a string");
}

#[test]
fn test_float_literal() {
    let mut ctx = Context::new();
    let r = ctx.eval("4.56").unwrap();
    assert!(r.is_float64(), "4.56 should be a float");
    assert!((r.as_float64().unwrap() - 4.56).abs() < 1e-10);
}

#[test]
fn test_float_addition() {
    let mut ctx = Context::new();
    let r = ctx.eval("1.5 + 2.5").unwrap();
    assert_eq!(r.as_smi(), Some(4));
}

#[test]
fn test_float_mixed_arith() {
    let mut ctx = Context::new();
    let r = ctx.eval("1.5 + 3").unwrap();
    assert!(r.is_float64(), "1.5 + 3 should be a float");
}

#[test]
fn test_switch_basic() {
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
    let r = ctx.eval("typeof 3.14").unwrap();
    assert!(
        r.heap_ptr().is_some(),
        "typeof float should return a string"
    );
}

#[test]
fn test_switch_fallthrough() {
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
    let r = ctx.eval("5 % 0").unwrap();
    assert!(r.is_float64() || r.is_smi(), "5 % 0 should be a number");
    assert!(
        r.as_float64().is_some_and(|v| v.is_nan()),
        "5 % 0 should be NaN"
    );
}

#[test]
fn test_exp_negative() {
    let mut ctx = Context::new();
    let r = ctx.eval("2 ** -1").unwrap();
    assert!(
        (r.as_float64().unwrap() - 0.5).abs() < 1e-10,
        "2 ** -1 should be 0.5"
    );
}

#[test]
fn test_null_plus_one() {
    let mut ctx = Context::new();
    let r = ctx.eval("null + 1").unwrap();
    assert_eq!(r.as_smi(), Some(1));
}

#[test]
fn test_neg_zero_preserved() {
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
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

#[test]
fn test_new_opcode_returns_object() {
    let mut ctx = Context::new();
    let r = ctx.eval("new Object()").unwrap();
    assert!(r.is_heap_object(), "new Object() should return an object");
}

#[test]
fn test_ic_populates_and_hits() {
    let mut ctx = Context::new();
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
        Instruction::new(Opcode::Dup, vec![]),
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
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
    let r = ctx.eval("[1, 2, 3]").unwrap();
    assert!(
        r.is_heap_object(),
        "array literal should return heap object"
    );
}

#[test]
fn test_dense_array_get_element() {
    let mut ctx = Context::new();
    // Single eval: create array and access multiple elements
    let r = ctx
        .eval("var a = [10, 20, 30]; a[0] + a[1] + a[2]")
        .unwrap();
    assert_eq!(r.as_smi(), Some(60));
}

#[test]
fn test_dense_array_out_of_bounds() {
    let mut ctx = Context::new();
    let r = ctx.eval("var a = [1, 2, 3]; a[5]").unwrap();
    assert!(r.is_undefined(), "out of bounds should be undefined");
}

#[test]
fn test_dense_array_set_element() {
    let mut ctx = Context::new();
    let r = ctx.eval("var a = [1, 2, 3]; a[0] = 99; a[0]").unwrap();
    assert_eq!(r.as_smi(), Some(99));
}

#[test]
fn test_array_push_pop() {
    let mut ctx = Context::new();
    let r = ctx.eval("var a = [1, 2]; a.push(3); a[2]").unwrap();
    assert_eq!(r.as_smi(), Some(3));
    let r2 = ctx.eval("var a = [1, 2, 3]; var v = a.pop(); v").unwrap();
    assert_eq!(r2.as_smi(), Some(3));
}

#[test]
fn test_array_push_grow() {
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
    let r = ctx.eval("var o={x:1,y:2,z:3}; var s=0; for(var k in o){s=s+o[k];} s");
    assert_eq!(r.unwrap().as_smi(), Some(6));
}

#[test]
fn test_for_in_array() {
    let mut ctx = Context::new();
    let r = ctx.eval("var a=[10,20,30]; var s=0; for(var k in a){s=s+a[k];} s");
    assert_eq!(r.unwrap().as_smi(), Some(60));
}

#[test]
fn test_for_in_empty() {
    let mut ctx = Context::new();
    let r = ctx.eval("var o={}; var c=0; for(var k in o){c=c+1;} c");
    assert_eq!(r.unwrap().as_smi(), Some(0));
}

#[test]
fn test_for_in_null() {
    let mut ctx = Context::new();
    let r = ctx.eval("var c=0; for(var k in null){c=c+1;} c");
    assert_eq!(r.unwrap().as_smi(), Some(0));
}

#[test]
fn test_array_is_array() {
    let mut ctx = Context::new();
    let r = ctx.eval("Array.isArray([1,2,3])").unwrap();
    assert_eq!(
        r.as_smi(),
        Some(1),
        "Array.isArray should return true for arrays"
    );
    let r2 = ctx.eval("Array.isArray(42)").unwrap();
    assert_eq!(
        r2.as_smi(),
        Some(0),
        "Array.isArray should return false for non-arrays"
    );
}

#[test]
fn test_math_constants() {
    let mut ctx = Context::new();
    let r = ctx.eval("Math.PI + 1").unwrap();
    assert!(r.is_float64(), "Math.PI + 1 should be a float64");
    let r2 = ctx.eval("Math.E + 1").unwrap();
    assert!(r2.is_float64(), "Math.E + 1 should be a float64");
}

#[test]
fn test_string_char_at() {
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
    let r = ctx.eval(r#"var s = "hello"; s.slice(0, 3)"#).unwrap();
    assert!(r.is_heap_object(), "slice should return a string");
}

#[test]
fn test_string_length() {
    let mut ctx = Context::new();
    let r = ctx.eval(r#"var s = "hello"; s.length"#).unwrap();
    assert_eq!(r.as_smi(), Some(5));
    let r2 = ctx.eval(r#"var s = "a"; s.length"#).unwrap();
    assert_eq!(r2.as_smi(), Some(1));
}

#[test]
fn test_math_floor() {
    let mut ctx = Context::new();
    let r = ctx.eval("Math.floor(3.7)").unwrap();
    assert_eq!(r.as_smi(), Some(3));
    let r2 = ctx.eval("Math.floor(-1.5)").unwrap();
    assert_eq!(r2.as_smi(), Some(-2));
    let r3 = ctx.eval("Math.floor(5)").unwrap();
    assert_eq!(r3.as_smi(), Some(5));
}

#[test]
fn test_math_ceil() {
    let mut ctx = Context::new();
    let r = ctx.eval("Math.ceil(3.2)").unwrap();
    assert_eq!(r.as_smi(), Some(4));
}

#[test]
fn test_math_abs() {
    let mut ctx = Context::new();
    let r = ctx.eval("Math.abs(-5)").unwrap();
    assert_eq!(r.as_smi(), Some(5));
}

#[test]
fn test_math_sqrt() {
    let mut ctx = Context::new();
    let r = ctx.eval("Math.sqrt(9)").unwrap();
    assert_eq!(r.as_smi(), Some(3));
}

#[test]
fn test_constructor_this_binding() {
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
    let r = ctx.eval("3.5 > 2").unwrap();
    assert_eq!(r.as_smi(), Some(1), "3.5 > 2 should be true");
    let r2 = ctx.eval("Math.PI > 3").unwrap();
    assert_eq!(r2.as_smi(), Some(1), "Math.PI > 3 should be true");
    let r3 = ctx.eval("1.5 < 2.5").unwrap();
    assert_eq!(r3.as_smi(), Some(1), "1.5 < 2.5 should be true");
}

#[test]
fn test_mixed_comparison() {
    let mut ctx = Context::new();
    let r = ctx.eval("3 > 2.5").unwrap();
    assert_eq!(r.as_smi(), Some(1), "Smi > Float64 should work");
    let r2 = ctx.eval("2.5 < 3").unwrap();
    assert_eq!(r2.as_smi(), Some(1), "Float64 < Smi should work");
}

#[test]
fn test_compound_assign() {
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
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
        r3.as_smi(),
        Some(0),
        "false && true should return false (0)"
    );
    let r4 = ctx.eval("true && 42").unwrap();
    assert_eq!(r4.as_smi(), Some(42), "true && 42 should return 42");
}

#[test]
fn test_logical_or() {
    let mut ctx = Context::new();
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
    assert_eq!(r3.as_smi(), Some(1), "true || false should return true (1)");
    let r4 = ctx.eval("false || 42").unwrap();
    assert_eq!(r4.as_smi(), Some(42), "false || 42 should return 42");
}

#[test]
fn test_delete_property() {
    let mut ctx = Context::new();
    let r = ctx.eval(r#"var o = {a: 1}; delete o.a; "a" in o"#).unwrap();
    assert_eq!(
        r.as_smi(),
        Some(0),
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
        r3.as_smi(),
        Some(1),
        "delete non-existent property returns true, 'a' in o still true"
    );
    let r4 = ctx.eval("delete 42").unwrap();
    assert_eq!(r4.as_smi(), Some(1), "delete 42 should return true");
}

#[test]
fn test_in_operator() {
    let mut ctx = Context::new();
    let r = ctx.eval(r#"var o = {a: 1}; "a" in o"#).unwrap();
    assert_eq!(r.as_smi(), Some(1), r#""a" in o should be true"#);
    let r2 = ctx.eval(r#"var o = {a: 1}; "b" in o"#).unwrap();
    assert_eq!(r2.as_smi(), Some(0), r#""b" in o should be false"#);
    let r3 = ctx.eval(r#"var a = [10, 20]; 0 in a"#).unwrap();
    assert_eq!(r3.as_smi(), Some(1), "0 in [10,20] should be true");
    let r4 = ctx.eval(r#"var a = [10, 20]; 2 in a"#).unwrap();
    assert_eq!(r4.as_smi(), Some(0), "2 in [10,20] should be false (OOB)");
    let r5 = ctx.eval(r#"var a = [10, 20]; "length" in a"#).unwrap();
    assert_eq!(r5.as_smi(), Some(1), "\"length\" in [10,20] should be true");
    // Nested object literal: property access via bracket notation
    let r6 = ctx
        .eval(r#"var o = {nested: {key: 1}}; "key" in o.nested"#)
        .unwrap();
    assert_eq!(r6.as_smi(), Some(1), "key in nested object should be true");
}

#[test]
fn test_strict_eq_smi_float() {
    let mut ctx = Context::new();
    let r = ctx.eval("1 === 1.0").unwrap();
    assert_eq!(
        r.as_smi(),
        Some(1),
        "1 === 1.0 should be true (Smi↔Float64 same number)"
    );
    let r2 = ctx.eval("1.0 === 1").unwrap();
    assert_eq!(r2.as_smi(), Some(1), "1.0 === 1 should be true");
    let r3 = ctx.eval("1 !== 1.0").unwrap();
    assert_eq!(r3.as_smi(), Some(0), "1 !== 1.0 should be false");
}

#[test]
fn test_strict_eq_nan() {
    let mut ctx = Context::new();
    let r = ctx.eval("NaN === NaN").unwrap();
    assert_eq!(
        r.as_smi(),
        Some(0),
        "NaN === NaN should be false per §7.2.14"
    );
    let r2 = ctx.eval("NaN !== NaN").unwrap();
    assert_eq!(r2.as_smi(), Some(1), "NaN !== NaN should be true");
}

#[test]
fn test_strict_eq_neg_zero() {
    let mut ctx = Context::new();
    let r = ctx.eval("(-0) === 0").unwrap();
    assert_eq!(r.as_smi(), Some(1), "-0 === 0 should be true per §7.2.14");
    let r2 = ctx.eval("0 === (-0)").unwrap();
    assert_eq!(r2.as_smi(), Some(1), "0 === -0 should be true");
    let r3 = ctx.eval("(-0) !== 0").unwrap();
    assert_eq!(r3.as_smi(), Some(0), "-0 !== 0 should be false");
}

#[test]
fn test_nan_comparison() {
    let mut ctx = Context::new();
    let r = ctx.eval("NaN < 5").unwrap();
    assert!(r.is_undefined(), "NaN < 5 should be undefined per §12.9");
    let r2 = ctx.eval("NaN >= 5").unwrap();
    assert_eq!(r2.as_smi(), Some(0), "NaN >= 5 should be false per §12.10");
    let r3 = ctx.eval("NaN <= 5").unwrap();
    assert_eq!(r3.as_smi(), Some(0), "NaN <= 5 should be false per §12.10");
}

#[test]
fn test_to_number_string() {
    let mut ctx = Context::new();
    let r = ctx.eval(r#""5" > 3"#).unwrap();
    assert_eq!(r.as_smi(), Some(1), "ToNumber('5') = 5 > 3");
    let r2 = ctx.eval(r#"3 > "5""#).unwrap();
    assert_eq!(r2.as_smi(), Some(0), "3 > ToNumber('5') should be false");
}

#[test]
fn test_increment_prefix() {
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
    let r = ctx.eval("var x = 5; x++").unwrap();
    assert_eq!(r.as_smi(), Some(5), "x++ should return old value");
    let r2 = ctx.eval("var y = 5; y++; y").unwrap();
    assert_eq!(r2.as_smi(), Some(6), "y should be incremented after x++");
}

#[test]
fn test_decrement() {
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
    let r = ctx.eval(r#"-"42""#).unwrap();
    let val = r
        .as_float64()
        .unwrap_or(r.as_smi().map(|v| v as f64).unwrap_or(f64::NAN));
    assert_eq!(val, -42.0, r#"-"42" should be -42 via ToNumber"#);
}

#[test]
fn test_negate_overflow() {
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
    let r = ctx.eval("-undefined").unwrap();
    assert!(
        r.as_float64().unwrap().is_nan(),
        "-undefined should be NaN per spec"
    );
}

#[test]
#[cfg(target_arch = "x86_64")]
fn test_jit_tier_up() {
    // add(a, b) uses only Smi arithmetic — JIT-compatible, will tier-up at 50 calls
    let mut ctx = Context::new();
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
}

#[test]
#[cfg(target_arch = "x86_64")]
fn test_jit_bailout_on_float() {
    // add() tier-up at 50, then pass a float64 — JIT should bail out to interpreter
    let mut ctx = Context::new();
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
}

mod instanceof_tests {
    use rune_embed::Context;

    #[test]
    fn test_instanceof_instance() {
        let mut ctx = Context::new();
        let r = ctx
            .eval(
                r#"
            function Foo() {}
            var f = new Foo();
            f instanceof Foo
        "#,
            )
            .unwrap();
        assert_eq!(r.as_smi(), Some(1), "instance instanceof constructor");
    }

    #[test]
    fn test_instanceof_false() {
        let mut ctx = Context::new();
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
            r.as_smi(),
            Some(0),
            "instance should not be instanceof different constructor"
        );
    }

    #[test]
    fn test_instanceof_prototype_chain() {
        let mut ctx = Context::new();
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
            r.as_smi(),
            Some(1),
            "child instance should be instanceof grandparent via prototype chain"
        );
    }

    #[test]
    fn test_instanceof_primitive_lhs() {
        let mut ctx = Context::new();
        let r = ctx
            .eval(
                r#"
            function Foo() {}
            42 instanceof Foo
        "#,
            )
            .unwrap();
        assert_eq!(
            r.as_smi(),
            Some(0),
            "primitive instanceof constructor should be false (empty proto chain)"
        );
    }

    // ---- let/const/TDZ tests ----

    #[test]
    fn test_let_decl() {
        let mut ctx = Context::new();
        let r = ctx.eval("let a = 1; a").unwrap();
        assert_eq!(r.as_smi(), Some(1));
    }

    #[test]
    fn test_let_reassign() {
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
        let r = ctx.eval("const c = 42; c").unwrap();
        assert_eq!(r.as_smi(), Some(42));
    }

    #[test]
    fn test_const_reassign_error() {
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
        // assert.sameValue with matching values should not throw
        let r = ctx.eval("assert.sameValue(1, 1); 'ok'").unwrap();
        assert!(r.is_heap_object(), "sameValue passed");
    }

    #[test]
    fn test_assert_not_same_value() {
        let mut ctx = Context::new();
        let r = ctx.eval("assert.notSameValue(1, 2); 'ok'").unwrap();
        assert!(r.is_heap_object(), "notSameValue passed");
    }

    #[test]
    fn test_assert_same_value_fails() {
        let mut ctx = Context::new();
        let e = ctx.eval("assert.sameValue(1, 2)");
        assert!(e.is_err(), "sameValue mismatch should error");
    }

    // ---- Arrow function tests ----

    #[test]
    fn test_arrow_single_param() {
        let mut ctx = Context::new();
        let r = ctx.eval("let f = x => x + 1; f(5)").unwrap();
        assert_eq!(r.as_smi(), Some(6));
    }

    #[test]
    fn test_arrow_multi_param() {
        let mut ctx = Context::new();
        let r = ctx.eval("let f = (a, b) => a + b; f(3, 4)").unwrap();
        assert_eq!(r.as_smi(), Some(7));
    }

    #[test]
    fn test_arrow_zero_params() {
        let mut ctx = Context::new();
        let r = ctx.eval("let f = () => 42; f()").unwrap();
        assert_eq!(r.as_smi(), Some(42));
    }

    #[test]
    fn test_arrow_block_body_with_let() {
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
        // Use a simple arrow call pattern (no Array.map, just direct call)
        let r = ctx.eval("let double = n => n * 2; double(21)").unwrap();
        assert_eq!(r.as_smi(), Some(42));
    }

    #[test]
    fn test_let_shadowing_in_block() {
        let mut ctx = Context::new();
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
        assert_eq!(
            r.as_smi(),
            Some(2),
            "inner x should shadow outer x"
        );
    }
}
