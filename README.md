# Rune

A experimental JavaScript engine written in Rust, designed for embedding in Rust applications with minimal overhead.

## Architecture

Rune is organized as a multi-crate workspace:

| Crate | Purpose |
|---|---|
| `rune_core` | Value types (tagged Smi/heap), GC (semi-space), shapes, objects, strings |
| `rune_bytecode` | Bytecode opcodes, instructions, program representation, block/analysis passes |
| `rune_parser` | JavaScript lexer and recursive-descent parser, bytecode emitter |
| `rune_interpreter` | Stack-based VM with call frames, generators, builtins, try/catch/finally |
| `rune_jit_baseline` | Baseline JIT compiler (copy-and-patch) for hot paths |
| `rune_builtins` | Built-in JS library implementations (arrays, strings, errors, JSON, math, maps, sets, promises) |
| `rune_regex` | Regular expression engine |
| `rune_module` | ES module loading and resolution |
| `rune_debugger` | Debugger interface (stepping, breakpoints, inspection) |
| `rune_embed` | High-level embedding API (`Context::eval`) for Rust applications |
| `rune_capi` | C FFI bindings for embedding in non-Rust hosts |
| `rune_cli` | CLI binary (`rune`) with REPL and Test262 runner |

## Features

- **GC**: Custom semi-space copying collector (Cheney-style). Optional MMTk backend behind `gc_mmtk` feature.
- **Value tagging**: 64-bit tagged values (Smi = i31, heap pointers, undefined, null)
- **Bytecode VM**: Stack-based interpreter with inline caches. Opcodes for literals, locals, globals, objects, control flow, functions, generators, try/catch/finally.
- **Try/catch/finally**: Full spec-compliant implementation — finally runs on normal completion, thrown exceptions, and return.
- **Global scope**: Variables persist across `eval()` calls via `HashMap<String, Value>`.
- **Generators**: `function*` with `yield` and `next()` / `return()` / `throw()`.
- **Builtins**: `print`, `String`, `Error`, `Test262Error`, `$DONOTEVALUATE` (extensible via `BuiltinFn` trait).
- **Test262**: Runner supports YAML frontmatter parsing, feature/flag-based skipping, harness injection.

## Usage

```rust
use rune_embed::Context;

let mut ctx = Context::new();
let result = ctx.eval("1 + 2").unwrap();
assert_eq!(result.as_smi(), Some(3));
```

### CLI

```sh
# Evaluate a JavaScript expression
rune eval 'print("hello world");'

# Run Test262 test suite
rune test262 language/expressions/addition
```

## Status

Rune is in early development. Phase 2 (parser, emitter, interpreter) is nearing completion. Phase 3 (JIT compiler) and Phase 4 (full ES2027 builtins) are in progress.

## License

MIT OR Apache-2.0
