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

#[test]
fn test_typeof_basic() {
    let mut ctx = Context::new();
    let r = ctx.eval(r#"typeof 42"#).unwrap();
    assert!(r.heap_ptr().is_some(), "typeof should return a string");
}

#[test]
fn test_float_literal() {
    let mut ctx = Context::new();
    let r = ctx.eval("3.14").unwrap();
    assert!(r.is_float64(), "3.14 should be a float");
    assert!((r.as_float64().unwrap() - 3.14).abs() < 1e-10);
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
    let r = ctx.eval(r#"
        let x = 2;
        let result = 0;
        switch (x) {
            case 1: result = 10; break;
            case 2: result = 20; break;
            default: result = 30;
        }
        result
    "#).unwrap();
    assert_eq!(r.as_smi(), Some(20));
}

#[test]
fn test_switch_default() {
    let mut ctx = Context::new();
    let r = ctx.eval(r#"
        let x = 99;
        let result = 0;
        switch (x) {
            case 1: result = 10; break;
            case 2: result = 20; break;
            default: result = 30;
        }
        result
    "#).unwrap();
    assert_eq!(r.as_smi(), Some(30));
}

#[test]
fn test_typeof_float() {
    let mut ctx = Context::new();
    let r = ctx.eval("typeof 3.14").unwrap();
    assert!(r.heap_ptr().is_some(), "typeof float should return a string");
}

#[test]
fn test_switch_fallthrough() {
    let mut ctx = Context::new();
    let r = ctx.eval(r#"
        let x = 1;
        let result = 0;
        switch (x) {
            case 1: result = 1;
            case 2: result = 2; break;
            default: result = 3;
        }
        result
    "#).unwrap();
    assert_eq!(r.as_smi(), Some(2));
}

#[test]
fn test_mod_zero_is_nan() {
    let mut ctx = Context::new();
    let r = ctx.eval("5 % 0").unwrap();
    assert!(r.is_float64() || r.is_smi(), "5 % 0 should be a number");
    assert!(r.as_float64().map_or(false, |v| v.is_nan()), "5 % 0 should be NaN");
}

#[test]
fn test_exp_negative() {
    let mut ctx = Context::new();
    let r = ctx.eval("2 ** -1").unwrap();
    assert!((r.as_float64().unwrap() - 0.5).abs() < 1e-10, "2 ** -1 should be 0.5");
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
    assert!(r.as_float64().unwrap().is_infinite(), "1 / -0 should be -Infinity");
    assert!(r.as_float64().unwrap().is_sign_negative(), "1 / -0 should be negative");
}

#[test]
fn test_prototype_chain_get() {
    let mut ctx = Context::new();
    let r = ctx.eval(r#"
        var animal = { speak: function() { return "generic"; } };
        var dog = Object.create(animal);
        dog.speak
    "#).unwrap();
    assert!(r.is_heap_object(), "should inherit speak from animal prototype");
}

#[test]
fn test_prototype_set_own_property() {
    let mut ctx = Context::new();
    let r = ctx.eval(r#"
        var animal = { x: 1 };
        var dog = Object.create(animal);
        dog.x = 2;
        dog.x
    "#).unwrap();
    assert_eq!(r.as_smi(), Some(2));
}

#[test]
fn test_prototype_shadow() {
    let mut ctx = Context::new();
    let r = ctx.eval(r#"
        var proto = { name: "proto" };
        var obj = Object.create(proto);
        obj.name = "own";
        obj.name
    "#).unwrap();
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
    let mut instrs = vec![];
    // obj1: {x: 1} — x is string pool index 0
    instrs.push(Instruction::new(Opcode::LoadSmi, vec![1]));
    instrs.push(Instruction::new(Opcode::NewObject, vec![1, 0]));
    // obj2: {x: 2, a: 0} — x=0, a=1 in string pool
    instrs.push(Instruction::new(Opcode::LoadSmi, vec![2]));
    instrs.push(Instruction::new(Opcode::LoadSmi, vec![0]));  // a's value
    instrs.push(Instruction::new(Opcode::NewObject, vec![2, 0, 1])); // x, a
    // obj3: {x: 3, a: 0, b: 0} — x=0, a=1, b=2 in string pool
    instrs.push(Instruction::new(Opcode::LoadSmi, vec![3]));
    instrs.push(Instruction::new(Opcode::LoadSmi, vec![0]));  // a's value
    instrs.push(Instruction::new(Opcode::LoadSmi, vec![0]));  // b's value
    instrs.push(Instruction::new(Opcode::NewObject, vec![3, 0, 1, 2])); // x, a, b
    // Access x on each in reverse stack order (LIFO)
    instrs.push(Instruction::new(Opcode::LoadStringConst, vec![0])); // key "x"
    instrs.push(Instruction::new(Opcode::LoadProperty, vec![]));     // obj3.x = 3
    instrs.push(Instruction::new(Opcode::Pop, vec![]));

    instrs.push(Instruction::new(Opcode::LoadStringConst, vec![0]));
    instrs.push(Instruction::new(Opcode::LoadProperty, vec![]));     // obj2.x = 2
    instrs.push(Instruction::new(Opcode::Pop, vec![]));

    instrs.push(Instruction::new(Opcode::LoadStringConst, vec![0]));
    instrs.push(Instruction::new(Opcode::LoadProperty, vec![]));     // obj1.x = 1 (last on stack)
    instrs.push(Instruction::new(Opcode::Return, vec![]));
    let mut prog = BytecodeProgram::new(instrs, vec!["x".to_string(), "a".to_string(), "b".to_string()], vec![]);
    prog.assign_ic_indices();
    let r = ctx.eval_bytecode(&prog).unwrap();
    assert_eq!(r.as_smi(), Some(1), "last access returns 1");
}

#[test]
fn test_ic_proto_inherited() {
    let mut ctx = Context::new();
    let r = ctx.eval(r#"
        var proto = {x: 99};
        var child = Object.create(proto);
        child.x
    "#).unwrap();
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
    ctx.eval(r#"
        var obj = {x: 42};
        obj.x; obj.x; obj.x; obj.x; obj.x;
        obj.x; obj.x; obj.x; obj.x; obj.x
    "#).unwrap();
    let stats1 = ctx.vm().ic_stats;
    assert_eq!(stats1.lookups, 10);
    assert_eq!(stats1.hits, 0);
    assert_eq!(stats1.misses, 10);

    // Second eval: same shape, same IC slots → all hits
    ctx.eval(r#"
        var obj = {x: 42};
        obj.x; obj.x; obj.x; obj.x; obj.x;
        obj.x; obj.x; obj.x; obj.x; obj.x
    "#).unwrap();
    let stats2 = ctx.vm().ic_stats;
    assert_eq!(stats2.lookups, 20);
    assert_eq!(stats2.hits, 10);
    assert_eq!(stats2.misses, 10);
}
