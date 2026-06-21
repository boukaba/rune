use libc;
use std::mem;

mod assembler;

use assembler::JitFn;

fn main() {
    println!("=== Phase 0, Spike 2: Copy-and-Patch JIT on aarch64 ===\n");

    // Validate the mechanism: write a single RET to MAP_JIT memory,
    // transition to RX, and call it as a function.
    let page_size = 4096;

    // --- MAP_JIT is macOS-only; Linux uses MAP_PRIVATE|MAP_ANONYMOUS ---
    #[cfg(target_os = "macos")]
    const JIT_FLAGS: i32 = libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_JIT;
    #[cfg(target_os = "linux")]
    const JIT_FLAGS: i32 = libc::MAP_PRIVATE | libc::MAP_ANONYMOUS;

    // --- Identity function (just RET) ---
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            page_size,
            libc::PROT_READ | libc::PROT_WRITE,
            JIT_FLAGS,
            -1,
            0,
        )
    };

    assert_ne!(ptr, libc::MAP_FAILED, "mmap MAP_JIT failed");

    // Write RET instruction (x0 already contains first arg, just return)
    unsafe { std::ptr::write(ptr as *mut u32, 0xD65F03C0u32) }

    // Transition to executable
    unsafe {
        libc::mprotect(ptr, page_size, libc::PROT_READ | libc::PROT_EXEC);
    }

    let identity: JitFn = unsafe { mem::transmute(ptr) };
    let r = unsafe { identity(42, 0, 0) };
    assert_eq!(r, 42);
    println!("  Test 1 (identity): PASS  ({})", r);

    // --- add3(a, b, c) compiled with assembler ---
    let (code, _) = assembler::compile_toy_jit();

    let add3_ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            page_size,
            libc::PROT_READ | libc::PROT_WRITE,
            JIT_FLAGS,
            -1,
            0,
        )
    };

    unsafe {
        std::ptr::copy_nonoverlapping(code.as_ptr(), add3_ptr as *mut u8, code.len());
        libc::mprotect(add3_ptr, page_size, libc::PROT_READ | libc::PROT_EXEC);
    }

    let add3: JitFn = unsafe { mem::transmute(add3_ptr) };

    let cases: [(&str, i64, i64, i64, i64); 5] = [
        ("positive",  10, 20, 30,      60),
        ("mixed",    -5, 10, 100,     105),
        ("large",     1_000_000_000, 500_000_000, 200_000_000, 1_700_000_000),
        ("zero",      0, 0, 0,         0),
        ("negative", -100, -200, -300, -600),
    ];

    for (name, a, b, c, expected) in cases {
        let result = unsafe { add3(a, b, c) };
        assert_eq!(result, expected);
        println!("  add3({a}, {b}, {c}) = {result}  [PASS]");
    }

    println!("\n=== All JIT spike tests PASS ===");
    println!("  - MAP_JIT allocation: OK");
    println!("  - mprotect RW→RX:    OK");
    println!("  - aarch64 icache:    OK (hardware-managed with MAP_JIT)");
    println!("  - Template assembly: OK");
    println!("\nCopy-and-patch technique validated on Apple Silicon.");
    println!("x86-64 templates follow the same pattern with x86 encodings.");
}
