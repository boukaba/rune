use mmtk::MMTKBuilder;
use mmtk::memory_manager;
use mmtk::util::ObjectReference;
use mmtk::util::opaque_pointer::*;
use spike_mmtk::object_model::{alloc_rune_object, get_shape_id, get_slot, set_shape_id, set_slot};
use spike_mmtk::{RuneVM, SINGLETON, mmtk};

/// MMTk Spike 1: ObjectModel binding validation.
/// Uses NoGC plan (no side metadata) to verify the VM trait plumbing.
/// MarkSweep with side metadata requires macOS kernel config work (Phase 1).
fn main() {
    println!("=== MMTk Spike 1: ObjectModel Binding ===");

    // --- Initialization (NoGC) ---
    let mut builder = MMTKBuilder::new();
    builder.set_option("plan", "NoGC");

    let mmtk_box = memory_manager::mmtk_init::<RuneVM>(&builder);
    if SINGLETON.set(mmtk_box).is_err() {
        panic!("SINGLETON already set");
    }

    println!("MMTk initialized with NoGC plan");

    // --- Create mutator ---
    let tls = VMMutatorThread(VMThread(OpaquePointer::UNINITIALIZED));
    let mut mutator = memory_manager::bind_mutator(mmtk(), tls);

    println!("Mutator created");

    // --- Test 1: Basic allocation ---
    println!("\n--- Test 1: Basic allocation ---");
    let obj1 = alloc_rune_object(&mut mutator, 2);
    unsafe {
        set_shape_id(obj1, 100);
        set_slot(obj1, 0, 42);
        set_slot(obj1, 1, 43);
    }
    unsafe {
        assert_eq!(get_slot(obj1, 0), 42);
        assert_eq!(get_slot(obj1, 1), 43);
        assert_eq!(get_shape_id(obj1), 100);
    }
    println!("  Basic allocation: PASS");

    // --- Test 2: Reference graph ---
    println!("\n--- Test 2: Reference graph ---");
    let parent = alloc_rune_object(&mut mutator, 2);
    let child = alloc_rune_object(&mut mutator, 2);
    unsafe {
        set_shape_id(parent, 1);
        set_shape_id(child, 2);
        set_slot(parent, 0, 123);
        set_slot(parent, 1, child.to_raw_address().as_usize() as u64);
        set_slot(child, 0, 456);
    }
    unsafe {
        let child_ref_addr = get_slot(parent, 1);
        assert_eq!(child_ref_addr, child.to_raw_address().as_usize() as u64);
        let child_obj = ObjectReference::from_raw_address(mmtk::util::Address::from_usize(
            child_ref_addr as usize,
        ))
        .expect("Invalid child ref");
        assert_eq!(get_slot(child_obj, 0), 456);
    }
    println!("  Reference graph: PASS");

    // --- Test 3: Multiple allocations ---
    println!("\n--- Test 3: Multiple allocations ---");
    let mut objects = Vec::new();
    for i in 0..1000 {
        let obj = alloc_rune_object(&mut mutator, 2);
        unsafe {
            set_shape_id(obj, 100 + i);
            set_slot(obj, 0, i);
        }
        objects.push(obj);
    }
    for (i, obj) in objects.iter().enumerate() {
        unsafe {
            assert_eq!(get_slot(*obj, 0), i as u64);
            assert_eq!(get_shape_id(*obj), 100 + i as u64);
        }
    }
    println!("  Multiple allocations: PASS");

    println!("\n=== All MMTk spike tests PASS ===");
    println!("(MarkSweep GC test deferred to Phase 1 — macOS side metadata needs config)");
}
