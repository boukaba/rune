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
- **Known gaps**: `async_reject` is_throw path not yet wired to generator throw, `.finally` pending case falls back to old .then behavior, RegExp: no match/search/split, `replaceAll` function replacement not yet implemented, class: no `static` methods, `this.prop++` not supported (Update only handles Identifier targets).

### Next Steps — v0.5 (ordered by leverage)
1. `static` methods
