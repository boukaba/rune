# ECMA-262 Reference Guide for Rune

> **Specification:** ECMAScript 2027 Language Specification (18th Edition)
> **URL:** https://tc39.es/ecma262/
> **Multipage:** https://tc39.es/ecma262/multipage/
> **Repo:** https://github.com/tc39/ecma262
> **Test Suite:** https://github.com/tc39/test262

---

## 1. How to Read the Spec

### Section Numbering
Every clause has a permanent section number (e.g., `sec-6.2.4` → `§6.2.4` in multipage). Links are stable across editions. Use the **multipage view** (`https://tc39.es/ecma262/multipage/`) for faster navigation.

### Algorithm Notation
Algorithms are written in a pseudocode step format:

```
1. Let obj be ? ToObject(arg).
2. Let len be ? LengthOfArrayLike(obj).
3. Repeat, while k < len:
   a. Let Pk be ! ToString(𝔽(k)).
   b. ...
```

- **`?`** prefix: the operation may throw — propagate any abrupt completion.
- **`!`** prefix: the operation is guaranteed not to throw.
- **`Assert:`**: an invariant the implementation must enforce (may be omitted in release builds).
- **`𝔽(x)`**: the Number value for x.
- **`ℤ(x)`**: the BigInt value for x.
- Ordinary text steps are sequential; indented sub-steps are inside a block.

### Completion Records
Every abstract operation returns a **Completion Record**: `{ [[Type]]: normal|return|throw|break|continue, [[Value]]: any, [[Target]]: label }`. `?` unwraps throws; `!` asserts normal.

### Grammar Notation
- **`|`** — alternatives (ordered: first match wins in lexical, spec-mandated in syntactic).
- **`⟦parameter⟧`** — grammatical parameter (e.g., `[~Yield]` means no `yield` allowed).
- **`[lookahead ∉ set]`** — negative lookahead restriction.
- **`~`** — line terminator restriction (`[no LineTerminator here]`).

### Internal Slots & Methods
Objects have **internal slots** `[[SlotName]]` (data) and **internal methods** `[[MethodName]]` (behaviour). Ordinary objects use the default definitions from `§10.1`; exotic objects override selected methods.

### Property Attributes
Every property has:
- `[[Value]]` — the stored value (data property)
- `[[Writable]]` — Boolean
- `[[Get]]` / `[[Set]]` — accessor property
- `[[Enumerable]]` — Boolean
- `[[Configurable]]` — Boolean

---

## 2. Spec Structure — Section Map for Rune

Below is the full ECMA-262 layout with the crate(s) responsible for implementing each part.

| Section | Title | Rune Crate(s) | Notes |
|---------|-------|---------------|-------|
| **§5** | Notational Conventions | all | Read first — defines the metalanguage |
| **§6** | ECMAScript Data Types and Values | `rune_core` | Types, Completion Records, List/Record specs |
| **§7** | Abstract Operations | `rune_core` | Type conversion (`ToNumber`, `ToString`, etc.) |
| **§8** | Syntax-Directed Operations | `rune_parser`, `rune_bytecode` | Cover grammar, early errors |
| **§9** | Executable Code and Execution Contexts | `rune_interpreter` | Execution stack, realms, environments |
| **§10** | Ordinary and Exotic Object Behaviours | `rune_core`, `rune_interpreter` | `[[Get]]`, `[[Set]]`, `[[Delete]]`, etc. |
| **§11** | ECMAScript Function Objects | `rune_interpreter`, `rune_jit_*` | Call/construct, `[[Call]]`, `[[Construct]]` |
| **§12** | Lexical Grammar | `rune_parser` | Tokenization, ASI, Unicode |
| **§13** | Syntax Grammar (Expressions) | `rune_parser` | Primary, LHS, update, unary, binary, etc. |
| **§14** | Syntax Grammar (Statements) | `rune_parser` | Block, if, for, while, try, switch, etc. |
| **§15** | Syntax Grammar (Functions & Classes) | `rune_parser`, `rune_bytecode` | Function/class declarations, `yield`, `await` |
| **§16** | Module Grammar | `rune_parser`, `rune_module` | import/export |
| **§17** | Built-in Objects (Object) | `rune_builtins` | Object constructor, prototype methods |
| **§18** | Function | `rune_builtins` | Function constructor, prototype |
| **§19** | Boolean | `rune_builtins` | Boolean constructor, prototype |
| **§20** | Symbol | `rune_builtins` | Symbol constructor, well-known symbols |
| **§21** | Error | `rune_builtins` | Error, TypeError, RangeError, etc. |
| **§22** | Number | `rune_builtins` | Number constructor, prototype, `Math` |
| **§23** | BigInt | `rune_builtins` | BigInt constructor, prototype |
| **§24** | String | `rune_builtins` | String constructor, prototype methods |
| **§25** | RegExp | `rune_regex`, `rune_builtins` | Regex parser, NFA/VM, String.prototype methods |
| **§26** | Indexed Collections | `rune_builtins` | Array, TypedArray, DataView |
| **§27** | Keyed Collections | `rune_builtins` | Map, Set, WeakMap, WeakSet |
| **§28** | Structured Data | `rune_builtins` | ArrayBuffer, SharedArrayBuffer, JSON, Atomics |
| **§29** | Control Abstraction | `rune_builtins`, `rune_interpreter` | Promise, Iterator, Generator, AsyncFunction |
| **§30** | Reflection | `rune_interpreter`, `rune_builtins` | Proxy, Reflect |
| **§31** | Modules | `rune_module` | Module records, linking, evaluation |
| **§32** | Memory Model | `rune_core` | Shared memory ordering (for Atomics) |
| **Annex A** | Grammar Summary | `rune_parser` | Consolidated lexical & syntactic grammar |
| **Annex B** | Additional ECMAScript Features for Web Browsers | `rune_builtins` | Legacy features (optional but expected) |
| **Annex C** | Host Layering Points | `rune_embed`, `rune_capi` | Host hooks, host-defined behaviour |

---

## 3. How to Use the Spec During Implementation

### Per-Crate Reading Order

#### `rune_core` — Start with §6 and §7
- **§6** defines the type system: `Value` = §6.1, `Object` = §6.1.7, `Completion Record` = §6.2.4.
- **§7** defines all abstract type conversion operations: `ToPrimitive` (§7.1.1), `ToBoolean` (§7.1.2), `ToNumber` (§7.1.3), `ToString` (§7.1.8), `ToObject` (§7.1.10), `ToPropertyKey` (§7.1.12).
- **§10** defines internal methods: `[[GetPrototypeOf]]`, `[[SetPrototypeOf]]`, `[[GetOwnProperty]]`, `[[DefineOwnProperty]]`, `[[Get]]`, `[[Set]]`, `[[Delete]]`, `[[OwnPropertyKeys]]`.

#### `rune_parser` — Start with §12, §13, §14, §15
- **§12** — Lexical grammar (tokens, Unicode, ASI at §12.10).
- **§13** — Expression grammar (precedence climbing).
- **§14** — Statement grammar (control flow, declarations).
- **§15** — Function/class definitions, generator/async syntax.
- **Annex A** — Full grammar summary for reference.

#### `rune_bytecode` — Start with §9, §15, §29
- **§9** — Execution contexts and environment records.
- **§15.5** — Generator function definitions (yield semantics).
- **§29.3** — Generator objects (`Generator.prototype.next`, `throw`, `return`).

#### `rune_interpreter` — Start with §9, §10, §11, §29
- **§9.4** — Execution context stack.
- **§9.5** — Environment records (declarative, function, global, module).
- **§10.1** — Ordinary object internal methods (the default implementations).
- **§11** — Function calls (`[[Call]]`, `[[Construct]]`).
- **§29** — Iterators, generators, async functions, promises.

#### `rune_builtins` — Implement per section group
- Implement one section at a time, starting with §17 (Object) → §18 (Function) → §19 (Boolean) → §21 (Error) → §22 (Number) → §24 (String) → §26 (Indexed Collections) → §27 (Keyed Collections) → §28 (Structured Data) → §29 (Control Abstraction).

#### `rune_regex` — Follow §25
- **§25.1** — RegExp pattern syntax.
- **§25.2** — RegExp objects (exec, test, flags, prototype methods).
- **Annex B.1.4** — Legacy RegExp features.

### Reading an Algorithm

Take `Object.prototype.toString()` (§20.1.3.6):

```
1. If *this* is *undefined*, return *"[object Undefined]"*.
2. If *this* is *null*, return *"[object Null]"*.
3. Let *O* be ! ToObject(*this*).
4. Let *isArray* be ? IsArray(*O*).
5. Let *builtinTag* be *"Object"*.
6. ...
```

Translation to Rust:
- "If this is undefined" → check `this` is the `undefined` value.
- "Let O be ! ToObject(this)" → call `ToObject(this)` which never throws.
- "Let isArray be ? IsArray(O)" → call `IsArray(O)` which may throw; propagate via `?`.
- Return a String value (not a String object).

---

## 4. Production Quality — What Conformance Means

§2 (Conformance) requires:

1. **All types, values, objects, properties, functions** described in the spec must be provided.
2. **Source text** must be interpreted according to the latest Unicode Standard and ISO/IEC 10646.
3. **Additional properties** are permitted on built-in objects, but **forbidden extensions** (Annex C) must not be implemented.
4. **Strict mode** (§4.3.2 / §8.2.3) — both strict and non-strict must be supported.
5. **Normative Optional** clauses (e.g., Annex B web features) — browsers must implement them; other embeddings may choose.
6. **Legacy** clauses are not core language but must be implemented unless also Normative Optional.

### What Rune Skips or Defers
- **Temporal** (not yet in the spec at the time of this writing — Stage 3 proposal).
- **Full Intl** (ECMA-402 is a separate spec; basic Intl coverage is acceptable).
- **`[[IsHTMLDDA]]`** (Annex B legacy — implement if running Test262 web tests).

---

## 5. Test262 Integration

Test262 is located at: https://github.com/tc39/test262

### Test Structure
```
test/
├── annexB/
├── built-ins/
├── harness/
├── language/
│   ├── asi/
│   ├── expressions/
│   ├── function-code/
│   ├── generators/
│   ├── modules/
│   ├── statements/
│   └── ...
├── staging/
└── intl402/
```

### How to Run
```rust
// In rune_cli/test262.rs
// - Parse the test YAML frontmatter
// - Determine expected result (pass/fail, error type, feature flags)
// - Run the test with Rune
// - Compare result to expected
// - Track pass/fail by section
```

### Key Features to Gate
Tests may require: `--features=class-fields`, `--features=regexp-v-flag`, etc. Read the `features:` list in the test's frontmatter and skip if Rune doesn't implement them yet.

---

## 6. Quick Reference — Most-Referenced Sections

| Purpose | Section | URL Fragment |
|---------|---------|-------------|
| Type system overview | §6.1 | `sec-ecmascript-data-types-and-values` |
| Completion Record | §6.2.4 | `sec-completion-record-specification-type` |
| ToPrimitive | §7.1.1 | `sec-toprimitive` |
| ToNumber | §7.1.3 | `sec-tonumber` |
| ToPropertyKey | §7.1.12 | `sec-topropertykey` |
| OrdinaryGet | §10.1.7.1 | `sec-ordinaryget` |
| OrdinarySet | §10.1.7.3 | `sec-ordinaryset` |
| [[Call]] | §11.2.1 | `sec-call` |
| Lexical grammar | §12 | `sec-lexical-grammar` |
| Automatic Semicolon Insertion | §12.10 | `sec-rules-of-automatic-semicolon-insertion` |
| Function definitions | §15.5 | `sec-function-definitions` |
| Generator definitions | §15.6 | `sec-generator-function-definitions` |
| Async function definitions | §15.9 | `sec-async-function-definitions` |
| Module grammar | §16.2 | `sec-modules` |
| typeof operator | §13.6.3 | `sec-typeof-operator` |
| Property access | §13.4 | `sec-property-accessors` |
| Object constructor | §17 | `sec-object-constructor` |
| Promise | §29.2 | `sec-promise-objects` |
| Proxy | §30.1 | `sec-proxy-object-internal-methods-and-internal-slots` |
| Generator | §29.3 | `sec-generator-objects` |
| GeneratorYield | §29.3.13 | `sec-generatoryield` |
| YieldStar | §29.3.14 | `sec-yieldstar-runtime-semantics-evaluation` |
| Atomics | §28.9 | `sec-atomics-object` |
| Memory Model | §32 | `sec-memory-model` |
| Annex B (web legacy) | Annex B | `sec-additional-ecmascript-features-for-web-browsers` |
| Forbidden Extensions | Annex C | `sec-forbidden-extensions` |
| Host Layering Points | Annex C (new) | `sec-host-layering-points` |
| Colophon (how spec is built) | Annex D | `sec-colophon` |

---

## 7. Grammar Validation

When implementing the parser, validate against **Annex A** — it contains the complete consolidated grammar. For any production, the spec provides:
- **Static Semantics**: Early error rules (e.g., duplicate parameter names).
- **Runtime Semantics**: Evaluation (produces a Value) or BindingInitialization.

Always implement both.

---

## 8. Checklist per Feature

For each language feature or built-in function:

- [ ] Read the spec section (algorithm steps, early errors)
- [ ] Identify all abstract operations called (trace `?` / `!` calls)
- [ ] Implement each abstract operation as a Rust function returning `Completion<Value>`
- [ ] Handle all edge cases (undefined/null arguments, missing properties, out-of-range indices)
- [ ] Pass the corresponding Test262 tests
- [ ] Run under the interpreter and JIT (results must match)
- [ ] Fuzz against V8 for differential comparison
