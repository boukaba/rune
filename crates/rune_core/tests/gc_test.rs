use rune_core::gc::SemiSpace;
use rune_core::object::JSObject;
use rune_core::shape::Shape;
use rune_core::string::HeapString;
use rune_core::value::Value;

#[test]
fn test_gc_survives_allocation() {
    let mut ss = SemiSpace::new();
    let shape = Shape::empty();
    let mut roots: Vec<u64> = vec![];

    for i in 0..1000 {
        let vals = vec![Value::smi(i)];
        let obj = JSObject::allocate(&mut ss, shape, &vals);
        roots.push(obj as u64);
    }

    for r in &mut roots {
        ss.push_root(r as *mut u64);
    }

    ss.collect();

    for (i, &r) in roots.iter().enumerate() {
        let obj = r as *mut JSObject;
        unsafe {
            let val = JSObject::get_slot(obj, 0);
            assert_eq!(val.as_smi(), Some(i as i32), "object {i} lost after GC");
        }
    }

    ss.clear_roots();
}

#[test]
fn test_gc_string_survives() {
    let mut ss = SemiSpace::new();
    let mut roots: Vec<u64> = vec![];

    for i in 0..200 {
        let s = format!("hello_{i}");
        let ptr = HeapString::allocate(&mut ss, &s);
        roots.push(ptr as u64);
    }

    for r in &mut roots {
        ss.push_root(r as *mut u64);
    }

    ss.collect();

    for (i, &r) in roots.iter().enumerate() {
        let ptr = r as *mut HeapString;
        unsafe {
            let s = HeapString::to_string(ptr);
            assert_eq!(s, format!("hello_{i}"), "string {i} corrupted after GC");
        }
    }

    ss.clear_roots();
}

#[test]
fn test_gc_reclaims_space() {
    let mut ss = SemiSpace::new();
    let shape = Shape::empty();

    for cycle in 0..5 {
        while ss.remaining() > 64 {
            let vals = vec![Value::smi(cycle)];
            let _obj = JSObject::allocate(&mut ss, shape, &vals);
        }
        ss.collect();
    }

    let v = vec![Value::smi(9999)];
    let obj = JSObject::allocate(&mut ss, shape, &v);
    assert!(!obj.is_null());
    unsafe {
        assert_eq!(JSObject::get_slot(obj, 0).as_smi(), Some(9999));
    }
}

#[test]
fn test_gc_object_graph() {
    let mut ss = SemiSpace::new();
    let shape = Shape::empty();

    let n = 100;
    let mut chain: Vec<u64> = Vec::with_capacity(n);

    for _ in 0..n {
        let inner = chain.first().copied().map(|p| Value::from_heap_ptr(p as *mut u8));
        let val = inner.unwrap_or(Value::smi(42));
        let obj = JSObject::allocate(&mut ss, shape, &[val]);
        chain.insert(0, obj as u64);
    }

    ss.push_root(&mut chain[0] as *mut u64);

    ss.collect();

    unsafe {
        let mut current = chain[0] as *mut JSObject;
        for depth in 0..n {
            assert!(!current.is_null(), "chain broken at depth {depth}");
            let val = JSObject::get_slot(current, 0);
            if let Some(next_ptr) = val.heap_ptr() {
                current = next_ptr as *mut JSObject;
            } else {
                assert_eq!(val.as_smi(), Some(42), "expected Smi(42) at end of chain");
                break;
            }
        }
    }

    ss.clear_roots();
}

#[test]
fn test_gc_idempotent() {
    let mut ss = SemiSpace::new();
    let shape = Shape::empty();

    ss.collect();
    ss.collect();
    ss.collect();

    for _ in 0..10 {
        let vals = vec![Value::smi(1)];
        let obj = JSObject::allocate(&mut ss, shape, &vals);
        let mut root = obj as u64;
        ss.push_root(&mut root as *mut u64);
        ss.collect();
        ss.collect();
        ss.clear_roots();
    }
}
