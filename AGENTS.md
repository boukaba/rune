# Instructions for AI coding agents

## Commit & Push
After completing any task or making meaningful progress, commit and push:
```sh
git add -A
git commit -m "description of changes"
git push
```

Exclude `ecma262.md` from commits (tracked locally only):
```sh
git rm --cached -f ecma262.md 2>/dev/null; true
```

Always use `git status` before committing to verify nothing unexpected is staged.

## Documentation discipline
After **every** task or meaningful progress, update these files before committing:
- `progress.md` — record what was done, test262 numbers if relevant, known gaps
- `README.md` — update the version table and feature list if a new feature landed
- `AGENTS.md` — update the anchored summary (Done / Known gaps / Next Steps sections)

Committing without updating docs hides progress from the project history. Always sync docs with code.

## Spec discipline
Before implementing ANY feature, always:
1. Read the relevant section in `ecma262.md` for the overview and spec links
2. Open every linked `https://tc39.es/ecma262/multipage/` URL via `webfetch` tool
3. Read the full algorithm steps — do NOT guess the spec
4. Cross-reference related sections (e.g. type conversion, internal methods, early errors)
5. Note subtle edge cases: type conversions, early errors, throw conditions, receiver handling
6. Only start implementing after you have the full spec picture

This applies to ALL phases: parser, emitter, bytecode, interpreter, builtins, JIT.

## Git user
This repo uses: `user.name = "boukaba"`, `user.email = "boukaba@users.noreply.github.com"

## Anchored Summary

### Goal
Ship a minimally viable JS engine for edge/serverless — cold-start wedge (2.8× vs Node) with enough stdlib to run real workloads. v0.4 = stdlib breadth (14 builtins). v0.5 = Promise + async patterns.

### Done — v0.4
- `Object.keys`/`values`/`entries` — shape properties, dense indices, char indices. test262: 56-62%.
- `Array.prototype`: includes, find, findIndex, some, every, flat, flatMap, sort (default lexicographic). test262: 5.6-75%.
- `String.prototype.replace`/`replaceAll` — string pattern only. test262: 16-22% (all regex/ToString gaps).
- `Number()` — ToNumber via ToPrimitive. test262: 132/340 (38.8%).
- `json_round_trip` benchmark: Rune cold-start 7.6ms vs Node 21ms → **2.8× faster**. Warm: Rune 0.79ms vs Node 0.146ms → 5.4× slower.

### Done — v0.5
- `async`/`await` — parser desugaring + generator reuse. 396/396 tests pass.
- `Promise` constructor + resolve/reject + `.then`/`.catch`/`.finally` + `Promise.resolve`/`.reject`/`.all`/`.race`
- Microtask queue — `.then` callbacks deferred via `drain_microtask_queue()`.
- Parser fix: reserved words valid as property names after `.`.
- `Array.prototype.indexOf` / `String.prototype.indexOf`
- **RegExp engine** — Thompson NFA + PikeVM, `TAG_REGEXP` GC type, `/pattern/flags` literal parsing, `RegExp.prototype.exec`/`.test`, regex replace with `$&`/``$` ``/`$'`/`$1..$n` expansion. 417/417 tests pass.
- **`class` syntax** — `class` declarations, expressions (named & anonymous), default constructor, method shorthand on prototype, `prototype` property linking via `StoreProperty` TAG_FUNC path in `do_store_property`. 7 integration tests. 423/423 tests pass.
- **Thenable unwrapping** — `Promise.resolve` detects objects with `.then` callable, creates a pending Promise, bridges via `PendingPromiseCtor` + `push_callback_call`. `.then` is called synchronously; fulfillment/rejection propagates through bridge functions. 3 integration tests. 425/425 tests pass.
- **RegExp prototype properties** — `source`, `flags`, `lastIndex` getters on `RegExp.prototype`, handled as own properties in `load_property_recursive`. `last_index` field added to RegExp struct (reused 4-byte padding). 3 integration tests.
- **RegExp function replacement** — `String.prototype.replace` supports function as replacement for regex pattern. Calls `fn(match, ...captures, offset, input)`, uses return value. Uses `PendingReplaceOp` state machine in Return handler. 2 integration tests. 429/429 tests pass.
- **`class` `extends` (heritage)** — prototype chain setup (`Child.prototype.__proto__ = Parent.prototype`), constructor `__proto__` linking for static inheritance (`Child.__proto__ = Parent`). 3 integration tests. 434/434 tests pass.
- **`class` `super()` calls** — `super(x, y)` in constructors: `Expr::Super` AST + parser, `LoadSuperclass` opcode (reads `Func::superclass` stored via `SetSuperclass` at class setup), `LoadThis` for receiver, `Call` to parent constructor. `func_ptr` field on Frame for superclass access. 4 integration tests. 438/438 tests pass.
- **`class` `super.prop` member access** — `super.method()` and `super.prop` resolve via `this.__proto__.__proto__` chain. `__proto__` read in `load_property_recursive` returns internal [[Prototype]] for TAG_OBJECT. 8 new tests. 448/448 tests pass.
- **Default derived constructor** — `class Child extends Parent { }` synthesizes `constructor(...args) { super(...args); }`. Fixed spread-Call `Expr::Super` handler bug (args were not being pushed). 3 new tests. 451/451 tests pass.
- **`instanceof` fix** — `instanceof` now works with builtin constructors (`Array`, `Promise`, `RegExp`) and class constructors. TAG_OBJECT builtin wrappers with `"prototype"` property are supported via shape lookup. 4 new tests. 456/456 tests pass.
- **`super.prop = val` assignment** — `super.prop = val` writes to `this` (child instance). `LoadThis` as receiver instead of obj on `Expr::Member(Expr::Super)` target. 2 new tests. 458/458 tests pass.
- **`static` methods** — `class Foo { static bar() { ... } }` supported. Static methods collected in emitter step 1, added to constructor after prototype link via `DefineProperty`. Func struct extended with `extra_props` field (lazily allocated JSObject for arbitrary properties on TAG_FUNC). `do_store_property`/`load_property_recursive`/`DefineProperty` all handle TAG_FUNC for non-prototype keys. GC traces `extra_props`. 4 new tests. 462/462 tests pass.
- **Getter/setter syntax** — `class Foo { get prop() { ... } set prop(v) { ... } }` supported. AST fields `is_getter`/`is_setter`, parser lookahead detection, `AccessorPair` GC type (TAG_ACCESSOR), `DefineAccessor` opcode, VM dispatch via `PendingAccessorCall` with `resolve_accessor_for_read` for getters and prototype-chain walk for setters. Fixed: inner-loop `continue` bug and `pending_accessor_call` depth guard. 6 new tests. 468/468 tests pass.
- **Compound assignment `super.prop += val`** — `super.prop += val` (and all compound assignment operators) now supported. The `Expr::CompoundAssign` handler desugars `super.a += rhs` differently from `o.a += rhs`: write-target setup emits `LoadThis` (child instance), read path emits `this.__proto__.__proto__` (superclass prototype), binary op, then `StoreProperty`. 1 new test. 469/469 tests pass.
- **Class private fields (`#`) runtime** — Full implementation from scaffold. Parser: `#name` and `#name = expr` in class body. AST: `PrivateField` struct (name, init, is_static, span). Emitter: `private_field_names` tracking in `Emitter`, `PrivateNameScope` opcode emission, slot-index resolution for `#name` member access, field init injected into constructor body. VM: `PrivateNameScope`/`DefinePrivateField`/`LoadPrivateProperty`/`StorePrivateProperty` handlers, `private_name_ids` on `Frame`, `next_private_name_id` counter on `Vm`. Func struct: 8-byte `private_name_ids` field (+8B → 80B total), getter/setter, GC tracing — propagated via `MakeFunction` so class methods retain access. `get_private_name_id` falls back to `Func.private_name_ids` when Frame's is null. 3 integration tests pass. 3 integration tests. 475/475 tests pass.
- **Known gaps**: RegExp: no match/search/split, `replaceAll` function replacement not yet implemented, `this.prop++` not supported (Update only handles Identifier targets), `let` + `new` in function body has a scoping bug. Static private fields and private methods not yet implemented. GC stress test `test_gc_during_jit_call_preserves_locals` broken since edc44b7 (getter/setter syntax).

### Next Steps — v0.5 (ordered by leverage)
1. `getter`/`setter` syntax — done
2. Compound assignment for `super.prop += val` — done
3. `class` private fields (`#`) — done
4. `String.prototype.match`/`search`/`split` for RegExp
