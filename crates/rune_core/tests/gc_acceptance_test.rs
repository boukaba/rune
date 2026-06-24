use rune_core::gc::SemiSpace;
use rune_core::object::JSObject;
use rune_core::shape::Shape;
use rune_core::value::Value;

/// Fill semispace with objects, keep a single root, verify GC works.
#[test]
fn test_semispace_full_gc() {
    let mut ss = SemiSpace::new();
    let shape = Shape::empty();
    let mut root = 0u64;

    // Allocate until nearly full; keep first object as root
    for i in 0.. {
        if ss.remaining() < 128 {
            break;
        }
        let vals = vec![Value::smi(i)];
        let obj = JSObject::allocate(&mut ss, shape, &vals);
        if i == 0 {
            root = obj as u64;
        }
    }

    // GC with root
    ss.push_root(&mut root as *mut u64);
    ss.collect();
    unsafe {
        let val = JSObject::get_slot(root as *mut JSObject, 0);
        assert_eq!(val.as_smi(), Some(0));
    }
    ss.clear_roots();

    // Should be able to allocate after GC
    let v = vec![Value::smi(777)];
    let obj = JSObject::allocate(&mut ss, shape, &v);
    unsafe {
        assert_eq!(JSObject::get_slot(obj, 0).as_smi(), Some(777));
    }
}

/// Fill twice with GC in between; roots from both survive.
#[test]
fn test_multiple_gc_cycles() {
    let mut ss = SemiSpace::new();
    let shape = Shape::empty();
    let mut root_a = 0u64;
    let mut root_b = 0u64;

    // First batch: fill and GC
    for i in 0.. {
        if ss.remaining() < 128 {
            break;
        }
        let vals = vec![Value::smi(i)];
        let obj = JSObject::allocate(&mut ss, shape, &vals);
        if i == 0 {
            root_a = obj as u64;
        }
    }

    ss.push_root(&mut root_a as *mut u64);
    ss.collect();
    ss.clear_roots();

    // Second batch: fill and GC
    for i in 0.. {
        if ss.remaining() < 128 {
            break;
        }
        let vals = vec![Value::smi(1000 + i)];
        let obj = JSObject::allocate(&mut ss, shape, &vals);
        if i == 0 {
            root_b = obj as u64;
        }
    }

    ss.push_root(&mut root_a as *mut u64);
    ss.push_root(&mut root_b as *mut u64);
    ss.collect();

    unsafe {
        assert_eq!(
            JSObject::get_slot(root_a as *mut JSObject, 0).as_smi(),
            Some(0)
        );
        assert_eq!(
            JSObject::get_slot(root_b as *mut JSObject, 0).as_smi(),
            Some(1000)
        );
    }
    ss.clear_roots();
}

/// Allocate without roots; objects reclaimed, memory stays bounded.
#[test]
fn test_no_root_reclamation() {
    let mut ss = SemiSpace::new();
    let shape = Shape::empty();

    for cycle in 0..5 {
        // Fill semispace with objects, no roots
        while ss.remaining() > 128 {
            let vals = vec![Value::smi(cycle)];
            let _obj = JSObject::allocate(&mut ss, shape, &vals);
        }
        ss.collect();
    }

    // After GC cycles, should still be able to allocate
    let v = vec![Value::smi(42)];
    let obj = JSObject::allocate(&mut ss, shape, &v);
    assert!(!obj.is_null());
    unsafe {
        assert_eq!(JSObject::get_slot(obj, 0).as_smi(), Some(42));
    }
}

/// GC pressure: rapid fill+GC in a tight loop
#[test]
fn test_rapid_gc() {
    let mut ss = SemiSpace::new();
    let shape = Shape::empty();

    for cycle in 0..50 {
        let mut root = 0u64;
        let mut count = 0;
        while ss.remaining() > 128 {
            let vals = vec![Value::smi(count + cycle * 1000)];
            let obj = JSObject::allocate(&mut ss, shape, &vals);
            if count == 0 {
                root = obj as u64;
            }
            count += 1;
        }
        ss.push_root(&mut root as *mut u64);
        ss.collect();
        // Verify root survived with correct value
        unsafe {
            let val = JSObject::get_slot(root as *mut JSObject, 0);
            assert_eq!(val.as_smi(), Some(cycle * 1000));
        }
        ss.clear_roots();
    }

    // Post-stress: fresh allocation
    let v = vec![Value::smi(9999)];
    let obj = JSObject::allocate(&mut ss, shape, &v);
    unsafe {
        assert_eq!(JSObject::get_slot(obj, 0).as_smi(), Some(9999));
    }
}

/// Multiple roots across fills
#[test]
fn test_kept_roots_across_generations() {
    let mut ss = SemiSpace::new();
    let shape = Shape::empty();
    let mut roots: Vec<u64> = Vec::new();

    for generation in 0..10 {
        // Allocate one root per generation
        let vals = vec![Value::smi(generation)];
        let obj = JSObject::allocate(&mut ss, shape, &vals);
        roots.push(obj as u64);

        if ss.remaining() < 1024 {
            // Register all accumulated roots and GC
            for r in &mut roots {
                ss.push_root(r as *mut u64);
            }
            ss.collect();
            ss.clear_roots();
        }
    }

    // Final GC
    for r in &mut roots {
        ss.push_root(r as *mut u64);
    }
    ss.collect();

    for (i, &r) in roots.iter().enumerate() {
        unsafe {
            let val = JSObject::get_slot(r as *mut JSObject, 0);
            assert_eq!(val.as_smi(), Some(i as i32), "generation {i} root lost");
        }
    }
    ss.clear_roots();
}
