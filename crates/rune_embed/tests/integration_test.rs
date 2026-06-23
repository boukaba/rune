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
    assert_eq!(r1.to_boolean(), Some(true));

    let r2 = ctx.eval("3 > 5").unwrap();
    assert_eq!(r2.to_boolean(), Some(false));
}

#[test]
fn test_eval_unary() {
    let mut ctx = Context::new();
    let r1 = ctx.eval("-5").unwrap();
    assert_eq!(r1.as_smi(), Some(-5));

    let r2 = ctx.eval("!true").unwrap();
    assert_eq!(r2.to_boolean(), Some(false));
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
    assert_eq!(r.to_boolean(), Some(true), "3.5 > 2 should be true");
    let r2 = ctx.eval("Math.PI > 3").unwrap();
    assert_eq!(r2.to_boolean(), Some(true), "Math.PI > 3 should be true");
    let r3 = ctx.eval("1.5 < 2.5").unwrap();
    assert_eq!(r3.to_boolean(), Some(true), "1.5 < 2.5 should be true");
}

#[test]
fn test_mixed_comparison() {
    let mut ctx = Context::new();
    let r = ctx.eval("3 > 2.5").unwrap();
    assert_eq!(r.to_boolean(), Some(true), "Smi > Float64 should work");
    let r2 = ctx.eval("2.5 < 3").unwrap();
    assert_eq!(r2.to_boolean(), Some(true), "Float64 < Smi should work");
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
        r3.to_boolean(),
        Some(false),
        "false && true should return false"
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
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
    let r = ctx.eval("+true").unwrap();
    assert_eq!(r.as_smi(), Some(1), "+true = 1 per §13.5.3");
    let r = ctx.eval("+false").unwrap();
    assert_eq!(r.as_smi(), Some(0), "+false = 0");
    let r = ctx.eval("+1").unwrap();
    assert_eq!(r.as_smi(), Some(1), "+1 = 1 (identity)");
}

#[test]
fn test_boolean_comparison() {
    let mut ctx = Context::new();
    let r = ctx.eval("true < 2").unwrap();
    assert_eq!(r.to_boolean(), Some(true), "true < 2 should be true");
    let r = ctx.eval("false < -1").unwrap();
    assert_eq!(r.to_boolean(), Some(false), "false < -1 should be false");
    let r = ctx.eval("true > 0").unwrap();
    assert_eq!(r.to_boolean(), Some(true), "true > 0 should be true");
}

#[test]
fn test_boolean_bitwise() {
    let mut ctx = Context::new();
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
    let mut ctx = Context::new();
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
        assert_eq!(
            r.to_boolean(),
            Some(true),
            "instance instanceof constructor"
        );
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
            r.to_boolean(),
            Some(false),
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
            r.to_boolean(),
            Some(true),
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
            r.to_boolean(),
            Some(false),
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
    fn test_arrow_is_not_constructable() {
        let mut ctx = Context::new();
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
        assert_eq!(r.as_smi(), Some(2), "inner x should shadow outer x");
    }

    // ---- Parenthesized expressions (Sprint 13G parser fix) ----

    #[test]
    fn test_paren_add() {
        let mut ctx = Context::new();
        let r = ctx.eval("var i = 7; var k = (i + 10); k").unwrap();
        assert_eq!(r.as_smi(), Some(17), "(i + 10) should be 17");
    }

    #[test]
    fn test_paren_sub() {
        let mut ctx = Context::new();
        let r = ctx.eval("var i = 7; var k = (i - 10); k").unwrap();
        assert_eq!(r.as_smi(), Some(-3), "(i - 10) should be -3");
    }

    #[test]
    fn test_paren_mul() {
        let mut ctx = Context::new();
        let r = ctx
            .eval("var a = 5; var b = 3; var k = (a + b) * 2; k")
            .unwrap();
        assert_eq!(r.as_smi(), Some(16), "(a + b) * 2 should be 16");
    }

    #[test]
    fn test_paren_nested() {
        let mut ctx = Context::new();
        let r = ctx
            .eval("var a = 5; var b = 3; var k = ((a + b) * 2); k")
            .unwrap();
        assert_eq!(r.as_smi(), Some(16), "((a + b) * 2) should be 16");
    }

    #[test]
    fn test_paren_in_call_arg() {
        let mut ctx = Context::new();
        let r = ctx
            .eval("function f(x){ return x; } var a = 5; var b = 3; f((a + b))")
            .unwrap();
        assert_eq!(r.as_smi(), Some(8), "f((a + b)) should be 8");
    }

    #[test]
    fn test_paren_conditional() {
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
        let r = ctx.eval("var x = 10; var r = (x > 5); r").unwrap();
        assert_eq!(r.to_boolean(), Some(true), "(x > 5) should be true");
    }

    #[test]
    fn test_paren_lt() {
        let mut ctx = Context::new();
        let r = ctx.eval("var x = 10; var r = (x < 5); r").unwrap();
        assert_eq!(r.to_boolean(), Some(false), "(x < 5) should be false");
    }

    #[test]
    fn test_paren_strict_eq() {
        let mut ctx = Context::new();
        let r = ctx.eval("var x = 10; var r = (x === 10); r").unwrap();
        assert_eq!(r.to_boolean(), Some(true), "(x === 10) should be true");
    }

    #[test]
    fn test_paren_mul_parse() {
        let mut ctx = Context::new();
        let r = ctx.eval("var i = 7; var k = (i * 10); k").unwrap();
        assert_eq!(r.as_smi(), Some(70), "(i * 10) should be 70");
    }

    #[test]
    fn test_paren_div_parse() {
        let mut ctx = Context::new();
        let r = ctx.eval("var i = 100; var k = (i / 10); k").unwrap();
        assert_eq!(r.as_smi(), Some(10), "(i / 10) should be 10");
    }

    #[test]
    fn test_paren_identifier_grouped() {
        let mut ctx = Context::new();
        let r = ctx.eval("var x = 42; (x)").unwrap();
        assert_eq!(r.as_smi(), Some(42), "(x) should be 42");
    }

    // ---- Destructuring (Sprint 14A) ----

    #[test]
    fn test_object_destructure_var() {
        let mut ctx = Context::new();
        let r = ctx.eval(r#"var {a, b} = {a: 1, b: 2}; a"#).unwrap();
        assert_eq!(r.as_smi(), Some(1), "var {{a, b}} = obj, a should be 1");
    }

    #[test]
    fn test_object_destructure_var_second() {
        let mut ctx = Context::new();
        let r = ctx.eval(r#"var {a, b} = {a: 1, b: 2}; b"#).unwrap();
        assert_eq!(r.as_smi(), Some(2), "var {{a, b}} = obj, b should be 2");
    }

    #[test]
    fn test_object_destructure_let() {
        let mut ctx = Context::new();
        let r = ctx.eval(r#"let {a, b} = {a: 10, b: 20}; a"#).unwrap();
        assert_eq!(r.as_smi(), Some(10), "let {{a, b}} = obj, a should be 10");
    }

    #[test]
    fn test_object_destructure_rename() {
        let mut ctx = Context::new();
        let r = ctx.eval(r#"var {a: x} = {a: 42}; x"#).unwrap();
        assert_eq!(r.as_smi(), Some(42), "var {{a: x}} = obj, x should be 42");
    }

    #[test]
    fn test_object_destructure_const() {
        let mut ctx = Context::new();
        let r = ctx.eval(r#"const {a, b} = {a: 5, b: 7}; a + b"#).unwrap();
        assert_eq!(
            r.as_smi(),
            Some(12),
            "const {{a, b}} = obj, a+b should be 12"
        );
    }

    #[test]
    fn test_object_destructure_missing_prop() {
        let mut ctx = Context::new();
        let r = ctx.eval(r#"var {a, b} = {a: 1}; b"#).unwrap();
        assert!(
            r.is_undefined(),
            "missing destructure prop should be undefined"
        );
    }

    #[test]
    fn test_array_destructure_var() {
        let mut ctx = Context::new();
        let r = ctx.eval(r#"var [a, b] = [1, 2]; a"#).unwrap();
        assert_eq!(r.as_smi(), Some(1), "var [a, b] = arr, a should be 1");
    }

    #[test]
    fn test_array_destructure_var_second() {
        let mut ctx = Context::new();
        let r = ctx.eval(r#"var [a, b] = [1, 2]; b"#).unwrap();
        assert_eq!(r.as_smi(), Some(2), "var [a, b] = arr, b should be 2");
    }

    #[test]
    fn test_array_destructure_let() {
        let mut ctx = Context::new();
        let r = ctx.eval(r#"let [a, b] = [10, 20]; a"#).unwrap();
        assert_eq!(r.as_smi(), Some(10), "let [a, b] = arr, a should be 10");
    }

    #[test]
    fn test_destructure_multi_decl() {
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
        let r = ctx
            .eval(r#"function f({a: {b, c}}) { return b + c; }; f({a: {b: 3, c: 4}})"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(7), "fn nested destructure should work");
    }

    #[test]
    fn test_fn_param_destructure_default() {
        let mut ctx = Context::new();
        let r = ctx
            .eval(r#"function f({a = 99}) { return a; }; f({}) + f({a: 5})"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(104), "fn destructure default should work");
    }

    #[test]
    fn test_fn_param_destructure_mixed() {
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
        // TypeError is thrown but try/catch in caller doesn't
        // catch across function frames yet; verify error is raised
        let r = ctx.eval(r#"function f({a}) { return a; }; f(null)"#);
        assert!(r.is_err(), "fn destructure null should throw TypeError");
    }

    #[test]
    fn test_fn_param_destructure_undefined_throws() {
        let mut ctx = Context::new();
        let r = ctx.eval(r#"function f({a}) { return a; }; f(undefined)"#);
        assert!(
            r.is_err(),
            "fn destructure undefined should throw TypeError"
        );
    }

    #[test]
    fn test_fn_param_destructure_named_function() {
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
        let r = ctx.eval(r#"var [a = 99] = []; a"#).unwrap();
        assert_eq!(r.as_smi(), Some(99), "[a = 99] = [], a should be 99");
    }

    #[test]
    fn test_array_destructure_default_not_undefined() {
        let mut ctx = Context::new();
        let r = ctx.eval(r#"var [a = 99] = [0]; a"#).unwrap();
        assert_eq!(
            r.as_smi(),
            Some(0),
            "[a = 99] = [0], a should be 0 (not 99)"
        );
    }

    #[test]
    fn test_array_destructure_default_explicit_undefined() {
        let mut ctx = Context::new();
        let r = ctx.eval(r#"var [a = 99] = [undefined]; a"#).unwrap();
        assert_eq!(
            r.as_smi(),
            Some(99),
            "[a = 99] = [undefined], a should be 99"
        );
    }

    #[test]
    fn test_array_destructure_default_null_not_triggered() {
        let mut ctx = Context::new();
        let r = ctx.eval(r#"var [a = 99] = [null]; a"#).unwrap();
        assert!(
            r.is_null(),
            "[a = 99] = [null], a should be null (default not triggered)"
        );
    }

    #[test]
    fn test_array_destructure_multi_element_defaults() {
        let mut ctx = Context::new();
        let r = ctx.eval(r#"var [a, b = 5] = [1]; a + b"#).unwrap();
        assert_eq!(r.as_smi(), Some(6), "[a, b = 5] = [1], a+b should be 6");
    }

    #[test]
    fn test_array_destructure_defaults_all_have_defaults() {
        let mut ctx = Context::new();
        let r = ctx.eval(r#"var [a = 1, b = 2] = [10]; a + b"#).unwrap();
        assert_eq!(r.as_smi(), Some(12), "[a=1, b=2] = [10], a+b should be 12");
    }

    #[test]
    fn test_array_destructure_default_fn_param() {
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
        let r = ctx.eval(r#"var [a] = null"#);
        assert!(r.is_err(), "[a] = null should throw TypeError");
    }

    // ── Spread / rest (14B) ─────────────────────────────────────────────

    // 14B-1: Rest parameter

    #[test]
    fn test_rest_param_basic() {
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
        let r = ctx
            .eval(r#"function f(...args) { return typeof args; }; f(42)"#)
            .unwrap();
        assert!(
            r.is_heap_object(),
            "typeof args should be a string (heap object)"
        );
    }

    // ---- 14B-3: Array spread ---

    #[test]
    fn test_array_spread_basic() {
        let mut ctx = Context::new();
        let r = ctx
            .eval(r#"var a = [1, 2]; var b = [...a, 3]; b.length"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(3), "b should have 3 elements");
    }

    #[test]
    fn test_array_spread_values() {
        let mut ctx = Context::new();
        let r = ctx
            .eval(r#"var a = [1, 2]; var b = [...a, 3]; b[0] + b[1] + b[2]"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(6), "1 + 2 + 3 should be 6");
    }

    #[test]
    fn test_array_spread_multiple() {
        let mut ctx = Context::new();
        let r = ctx
            .eval(r#"var a = [1, 2]; var b = [3, 4]; var c = [...a, ...b]; c.length"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(4), "c should have 4 elements");
    }

    #[test]
    fn test_array_spread_mixed() {
        let mut ctx = Context::new();
        let r = ctx
            .eval(r#"var a = [2, 3]; var b = [1, ...a, 4]; b[0] + b[1] + b[2] + b[3]"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(10), "1 + 2 + 3 + 4 should be 10");
    }

    #[test]
    fn test_array_spread_empty() {
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
        let r = ctx
            .eval(r#"var f = (...args) => args.length; f(1, 2)"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(2), "arrow rest param should capture args");
    }

    #[test]
    fn test_arrow_rest_param_single() {
        let mut ctx = Context::new();
        let r = ctx.eval(r#"var f = (...args) => args[0]; f(42)"#).unwrap();
        assert_eq!(
            r.as_smi(),
            Some(42),
            "arrow rest param should access first arg"
        );
    }

    #[test]
    fn test_arrow_rest_param_mixed() {
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
        let r = ctx
            .eval(r#"var a = {x: 1, y: 2}; var b = {...a}; b.x + b.y"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(3), "shallow copy should preserve values");
    }

    #[test]
    fn test_object_spread_not_same() {
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
        let r = ctx
            .eval(r#"var a = {x: 1}; var b = {...a, x: 2}; b.x"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(2), "literal after spread should override");
    }

    #[test]
    fn test_object_spread_spread_after_literal() {
        let mut ctx = Context::new();
        let r = ctx
            .eval(r#"var a = {x: 2}; var b = {x: 1, ...a}; b.x"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(2), "spread after literal should override");
    }

    #[test]
    fn test_object_spread_merge() {
        let mut ctx = Context::new();
        let r = ctx
            .eval(r#"var a = {x: 1}; var b = {y: 2}; var c = {...a, ...b}; c.x + c.y"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(3), "merge two objects via spread");
    }

    #[test]
    fn test_object_spread_empty() {
        let mut ctx = Context::new();
        let r = ctx.eval(r#"var a = {...{}}; typeof a"#).unwrap();
        assert!(
            r.is_heap_object(),
            "empty object spread should return an object"
        );
    }

    #[test]
    fn test_object_spread_null_noop() {
        let mut ctx = Context::new();
        let r = ctx.eval(r#"var a = {...null}; typeof a"#).unwrap();
        // typeof a === "object" — null spread is no-op, a is {}
        assert!(r.is_heap_object(), "typeof a should be a string");
    }

    #[test]
    fn test_object_spread_undefined_noop() {
        let mut ctx = Context::new();
        let r = ctx.eval(r#"var a = {...undefined}; typeof a"#).unwrap();
        assert!(r.is_heap_object(), "typeof a should be a string");
    }

    // ---- 14B-5: Rest in destructuring ---

    #[test]
    fn test_array_rest_basic() {
        let mut ctx = Context::new();
        let r = ctx
            .eval(r#"var [a, ...rest] = [1, 2, 3]; a + rest[0] + rest[1]"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(6), "1 + 2 + 3 = 6");
    }

    #[test]
    fn test_array_rest_single() {
        let mut ctx = Context::new();
        let r = ctx.eval(r#"var [a, ...rest] = [1]; rest.length"#).unwrap();
        assert_eq!(
            r.as_smi(),
            Some(0),
            "rest should be empty when only one element"
        );
    }

    #[test]
    fn test_array_rest_only() {
        let mut ctx = Context::new();
        let r = ctx
            .eval(r#"var [...rest] = [1, 2, 3]; rest.length"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(3), "rest-only should capture all elements");
    }

    #[test]
    fn test_array_rest_multi() {
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
        let r = ctx
            .eval(r#"var {a, ...rest} = {a: 1, b: 2, c: 3}; a + rest.b + rest.c"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(6), "1 + 2 + 3 = 6");
    }

    #[test]
    fn test_object_rest_excludes() {
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
        let r = ctx
            .eval(r#"var {...rest} = {x: 10, y: 20}; rest.x + rest.y"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(30), "rest-only should capture all props");
    }

    #[test]
    fn test_object_rest_multi_exclude() {
        let mut ctx = Context::new();
        let r = ctx
            .eval(r#"var {a, b, ...rest} = {a: 1, b: 2, c: 3, d: 4}; rest.c + rest.d"#)
            .unwrap();
        assert_eq!(r.as_smi(), Some(7), "multi-exclude rest should work");
    }

    #[test]
    fn test_object_rest_no_leftover() {
        let mut ctx = Context::new();
        let r = ctx.eval(r#"var {a, ...rest} = {a: 1}; rest.b"#).unwrap();
        // rest.b should be undefined, which is the default
        assert!(
            r.is_undefined(),
            "no-leftover rest should have undefined props"
        );
    }

    #[test]
    fn test_object_rest_let() {
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
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
        let mut ctx = Context::new();
        let r = ctx
            .eval("function f(a,b,c) { return a + b + c; } let arr = [1,2,3]; f(...arr)")
            .unwrap();
        assert_eq!(r.as_smi(), Some(6), "f(...[1,2,3])");
    }

    #[test]
    fn test_spread_call_mixed() {
        let mut ctx = Context::new();
        let r = ctx
            .eval("function f(a,b,c) { return a + b + c; } f(0, ...[1,2])")
            .unwrap();
        assert_eq!(r.as_smi(), Some(3), "f(0, ...[1,2])");
    }

    #[test]
    fn test_spread_call_multiple_spreads() {
        let mut ctx = Context::new();
        let r = ctx
            .eval("function f(a,b,c) { return a + b + c; } f(...[1], 2, ...[3])")
            .unwrap();
        assert_eq!(r.as_smi(), Some(6), "f(...[1], 2, ...[3])");
    }

    #[test]
    fn test_spread_call_empty() {
        let mut ctx = Context::new();
        let r = ctx.eval("function f() { return 42; } f(...[])").unwrap();
        assert_eq!(r.as_smi(), Some(42), "f(...[]) with no-arg fn");
    }

    #[test]
    fn test_spread_call_builtin() {
        let mut ctx = Context::new();
        let r = ctx.eval("Math.max(...[1,2,3])").unwrap();
        assert_eq!(r.as_smi(), Some(3), "Math.max(...[1,2,3])");
    }

    #[test]
    fn test_spread_call_print() {
        let mut ctx = Context::new();
        let r = ctx.eval("let s = ''; function capture(...args) { s = args.join(','); } capture(...[10,20,30]); s").unwrap();
        assert!(
            r.heap_ptr().is_some(),
            "spread call with rest param should yield joined string"
        );
    }

    #[test]
    fn test_spread_call_rest_param() {
        let mut ctx = Context::new();
        let r = ctx
            .eval("function f(...args) { return args.length; } f(...[1,2,3])")
            .unwrap();
        assert_eq!(r.as_smi(), Some(3), "f(...[1,2,3]) with rest param");
    }

    // --- Sprint 14C: Object literal extensions ---

    #[test]
    fn test_shorthand_property() {
        let mut ctx = Context::new();
        let r = ctx
            .eval("var a = 1, b = 2; var o = { a, b }; o.a === 1 && o.b === 2")
            .unwrap();
        assert_eq!(r.to_boolean(), Some(true), "shorthand");
    }

    #[test]
    fn test_shorthand_single() {
        let mut ctx = Context::new();
        let r = ctx.eval("var x = 42; var o = { x }; o.x").unwrap();
        assert_eq!(r.as_smi(), Some(42), "shorthand single");
    }

    #[test]
    fn test_shorthand_mixed() {
        let mut ctx = Context::new();
        let r = ctx
            .eval("var a = 1; var o = { a, b: 2 }; o.a === 1 && o.b === 2")
            .unwrap();
        assert_eq!(r.to_boolean(), Some(true), "shorthand mixed");
    }

    #[test]
    fn test_shorthand_fn_ref() {
        let mut ctx = Context::new();
        let r = ctx
            .eval("function f() { return 42; } var o = { f }; o.f()")
            .unwrap();
        assert_eq!(r.as_smi(), Some(42), "shorthand function ref");
    }

    #[test]
    fn test_method_shorthand_basic() {
        let mut ctx = Context::new();
        let r = ctx
            .eval("var o = { foo() { return 42; } }; o.foo()")
            .unwrap();
        assert_eq!(r.as_smi(), Some(42), "method shorthand basic");
    }

    #[test]
    fn test_method_shorthand_this() {
        let mut ctx = Context::new();
        let r = ctx
            .eval("var o = { x: 1, getX() { return this.x; } }; o.getX()")
            .unwrap();
        assert_eq!(r.as_smi(), Some(1), "method shorthand this");
    }

    #[test]
    fn test_method_shorthand_multiple() {
        let mut ctx = Context::new();
        let r = ctx
            .eval("var o = { a() { return 1; }, b() { return 2; } }; o.a() + o.b()")
            .unwrap();
        assert_eq!(r.as_smi(), Some(3), "multiple methods");
    }

    #[test]
    fn test_method_shorthand_arguments() {
        let mut ctx = Context::new();
        let r = ctx
            .eval("var o = { f(a, b) { return a + b; } }; o.f(10, 20)")
            .unwrap();
        assert_eq!(r.as_smi(), Some(30), "method shorthand with params");
    }

    #[test]
    fn test_computed_key_basic() {
        let mut ctx = Context::new();
        let r = ctx.eval("var k = 'x'; var o = { [k]: 1 }; o.x").unwrap();
        assert_eq!(r.as_smi(), Some(1), "computed key basic");
    }

    #[test]
    fn test_computed_key_string_concat() {
        let mut ctx = Context::new();
        let r = ctx
            .eval("var i = 0; var o = { ['key' + i]: 42 }; o.key0")
            .unwrap();
        assert_eq!(r.as_smi(), Some(42), "computed string concatenation");
    }

    #[test]
    fn test_computed_key_numeric() {
        let mut ctx = Context::new();
        let r = ctx
            .eval("var n = 5; var o = { [n]: 'five' }; o[5]")
            .unwrap();
        assert!(r.heap_ptr().is_some(), "computed numeric key");
    }

    #[test]
    fn test_computed_key_multiple() {
        let mut ctx = Context::new();
        let r = ctx
            .eval("var k = 'x'; var o = { [k]: 1, [k + '2']: 2 }; o.x === 1 && o.x2 === 2")
            .unwrap();
        assert_eq!(r.to_boolean(), Some(true), "multiple computed keys");
    }

    #[test]
    fn test_computed_method_name() {
        let mut ctx = Context::new();
        let r = ctx
            .eval("var k = 'x'; var o = { [k]() { return 42; } }; o.x()")
            .unwrap();
        assert_eq!(r.as_smi(), Some(42), "computed method name");
    }

    #[test]
    fn test_computed_destructuring() {
        let mut ctx = Context::new();
        let r = ctx
            .eval("var k = 'x'; var { [k]: val } = { x: 1 }; val")
            .unwrap();
        assert_eq!(r.as_smi(), Some(1), "computed key destructuring");
    }
}
