//! F1 regression tests: Func flags-word bit plan.
//!
//! The 4-byte flags word packs kind bits (low 5) + biased module index
//! (bits 5..=31). These tests pin the layout so future slices (A3 class-call
//! checks, strict poison, module resolution) can rely on it: flags must be
//! mutually independent, and setting module_mi must preserve kind bits and
//! vice versa.

use rune_core::function::{
    FUNC_FLAG_ARROW, FUNC_FLAG_ASYNC, FUNC_FLAG_CLASS_CTOR, FUNC_FLAG_GENERATOR, FUNC_FLAG_STRICT,
    Func,
};
use rune_core::gc::SemiSpace;

fn fresh_func(ss: &mut SemiSpace, is_arrow: bool) -> *mut Func {
    Func::allocate(ss, 0, std::ptr::null(), is_arrow, std::ptr::null_mut())
}

#[test]
fn test_flags_default_off_module_none() {
    let mut ss = SemiSpace::new();
    let f = fresh_func(&mut ss, false);
    unsafe {
        assert!(!Func::is_arrow(f));
        assert!(!Func::is_strict(f));
        assert!(!Func::is_class_constructor(f));
        assert!(!Func::is_generator_fn(f));
        assert!(!Func::is_async_fn(f));
        assert_eq!(Func::module_mi(f), -1, "fresh Func has no module");
    }
}

#[test]
fn test_allocate_arrow_bit() {
    let mut ss = SemiSpace::new();
    let f = fresh_func(&mut ss, true);
    unsafe {
        assert!(Func::is_arrow(f));
        assert!(!Func::is_class_constructor(f));
        assert_eq!(Func::module_mi(f), -1);
    }
}

#[test]
fn test_kind_flags_independent() {
    let mut ss = SemiSpace::new();
    let f = fresh_func(&mut ss, false);
    unsafe {
        Func::set_class_constructor(f, true);
        Func::set_generator_fn(f, true);
        Func::set_async_fn(f, true);
        Func::set_strict(f, true);
        assert!(Func::is_class_constructor(f));
        assert!(Func::is_generator_fn(f));
        assert!(Func::is_async_fn(f));
        assert!(Func::is_strict(f));
        assert!(!Func::is_arrow(f), "arrow bit untouched by kind sets");
        assert_eq!(Func::module_mi(f), -1, "module still none");
        // Clearing one bit leaves the rest alone.
        Func::set_generator_fn(f, false);
        assert!(!Func::is_generator_fn(f));
        assert!(Func::is_class_constructor(f));
        assert!(Func::is_async_fn(f));
        assert!(Func::is_strict(f));
    }
}

#[test]
fn test_module_mi_round_trip_preserves_flags() {
    let mut ss = SemiSpace::new();
    let f = fresh_func(&mut ss, true);
    unsafe {
        Func::set_generator_fn(f, true);
        for mi in [-1, 0, 1, 42, 1_000_000] {
            Func::set_module_mi(f, mi);
            assert_eq!(Func::module_mi(f), mi, "module_mi round trip for {mi}");
            assert!(Func::is_arrow(f), "arrow survives set_module_mi({mi})");
            assert!(
                Func::is_generator_fn(f),
                "generator survives set_module_mi({mi})"
            );
        }
        // Kind-bit writes preserve a live module index.
        Func::set_module_mi(f, 7);
        Func::set_class_constructor(f, true);
        Func::set_strict(f, true);
        Func::set_async_fn(f, true);
        assert_eq!(Func::module_mi(f), 7, "module survives kind-bit writes");
        assert!(Func::is_class_constructor(f));
        assert!(Func::is_strict(f));
        assert!(Func::is_async_fn(f));
    }
}

#[test]
fn test_add_flags_masks_to_kind_bits() {
    let mut ss = SemiSpace::new();
    let f = fresh_func(&mut ss, false);
    unsafe {
        Func::set_module_mi(f, 3);
        // A wild high-bit word must not clobber the module index.
        Func::add_flags(f, 0xFFFF_FFFF);
        assert_eq!(Func::module_mi(f), 3, "add_flags never touches module bits");
        assert!(Func::is_arrow(f));
        assert!(Func::is_strict(f));
        assert!(Func::is_class_constructor(f));
        assert!(Func::is_generator_fn(f));
        assert!(Func::is_async_fn(f));
        // Low-bit constant sanity: exactly the documented five bits.
        assert_eq!(
            FUNC_FLAG_ARROW
                | FUNC_FLAG_STRICT
                | FUNC_FLAG_CLASS_CTOR
                | FUNC_FLAG_GENERATOR
                | FUNC_FLAG_ASYNC,
            0x1F
        );
    }
}
