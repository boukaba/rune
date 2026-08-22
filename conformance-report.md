# test262 Conformance Baseline (2026-08-22)

**Completed suites:** 168 · **PASS 3623 / 22257 = 16%** · SKIP 7481 (unsupported features)
**intl402 excluded** (ECMA-402 not implemented). 19 suites CRASH or HANG before completing — counts below are partial for those.

## Stability blockers (must fix before meaningful measurement)

- `built-ins/Temporal` — 2492 fails logged then death
- `language/statements/class` — 2171 fails logged then death
- `built-ins/Array` — 1842 fails logged then death
- `built-ins/Object` — 1225 fails logged then death
- `language/expressions` — 959 fails logged then death
- `annexB` — 437 fails logged then death
- `staging` — 281 fails logged then death
- `built-ins/ArrayBuffer` — 148 fails logged then death
- `language/expressions/class` — 126 fails logged then death
- `language/expressions/object` — 112 fails logged then death
- `built-ins/RegExp` — 94 fails logged then death
- `language/expressions/call` — 28 fails logged then death
- `language/expressions/async-function` — 18 fails logged then death
- `language/statements/async-function` — 15 fails logged then death
- `language/expressions/optional-chaining` — 7 fails logged then death
- `built-ins/Promise` — 5 fails logged then death
- `language/statements/for` — 1 fails logged then death
- `language/statements/try` — 1 fails logged then death
- `language/statements/continue` — 0 fails logged then death

## All suites (by failures)

| suite | pass% | fail | total |
|---|---|---|---|
| built-ins/Temporal | — | 2492 | ∞ |
| language/statements/class | — | 2171 | ∞ |
| built-ins/Array | — | 1842 | ∞ |
| built-ins/Object | — | 1225 | ∞ |
| language/statements/for-await-of | 0% | 1064 | 1234 |
| language/expressions | — | 959 | ∞ |
| built-ins/String | 33% | 708 | 1223 |
| language/expressions/dynamic-import | 0% | 564 | 1060 |
| language/statements/for-of | 7% | 557 | 751 |
| language/expressions/async-generator | 4% | 489 | 623 |
| annexB | — | 437 | ∞ |
| built-ins/DataView | 7% | 397 | 561 |
| built-ins/Function | 13% | 361 | 509 |
| built-ins/Iterator | 5% | 339 | 514 |
| language/expressions/assignment | 2% | 331 | 485 |
| staging | — | 281 | ∞ |
| language/expressions/compound-assignment | 28% | 258 | 454 |
| language/statements/async-generator | 4% | 248 | 301 |
| language/eval-code | 20% | 244 | 347 |
| built-ins/Proxy | 4% | 239 | 311 |
| language/expressions/generators | 1% | 228 | 290 |
| language/statements/function | 32% | 220 | 451 |
| language/expressions/arrow-function | 8% | 212 | 343 |
| language/statements/generators | 6% | 208 | 266 |
| built-ins/Number | 26% | 199 | 340 |
| built-ins/Date | 43% | 181 | 594 |
| built-ins/Atomics | 2% | 169 | 390 |
| language/arguments-object | 27% | 156 | 263 |
| language/statements/with | 1% | 154 | 181 |
| built-ins/ArrayBuffer | — | 148 | ∞ |
| language/expressions/function | 29% | 134 | 264 |
| language/expressions/class | — | 126 | ∞ |
| built-ins/TypedArray | 0% | 116 | 1446 |
| built-ins/JSON | 15% | 113 | 165 |
| language/expressions/object | — | 112 | ∞ |
| built-ins/RegExp | — | 94 | ∞ |
| built-ins/Math | 34% | 93 | 327 |
| language/function-code | 40% | 86 | 217 |
| language/literals | 20% | 84 | 534 |
| built-ins/Reflect | 17% | 83 | 153 |
| language/identifiers | 26% | 80 | 268 |
| built-ins/AsyncDisposableStack | 0% | 77 | 104 |
| built-ins/SharedArrayBuffer | 5% | 76 | 104 |
| language/statements/let | 23% | 75 | 145 |
| built-ins/WeakMap | 31% | 73 | 141 |
| language/expressions/super | 18% | 70 | 94 |
| language/statements/const | 23% | 69 | 136 |
| language/statements/variable | 23% | 68 | 178 |
| built-ins/DisposableStack | 1% | 66 | 93 |
| language/statements/await-using | 1% | 60 | 94 |
| language/expressions/yield | 0% | 59 | 63 |
| built-ins/Map | 50% | 52 | 204 |
| built-ins/decodeURIComponent | 1% | 52 | 56 |
| built-ins/decodeURI | 1% | 51 | 55 |
| built-ins/ShadowRealm | 1% | 46 | 67 |
| built-ins/Uint8Array | 2% | 46 | 70 |
| built-ins/GeneratorPrototype | 9% | 44 | 61 |
| language/statements/for-in | 7% | 44 | 115 |
| language/statementList | 48% | 41 | 80 |
| language/types | 48% | 39 | 113 |
| built-ins/AsyncFromSyncIteratorPrototype | 0% | 38 | 38 |
| language/expressions/instanceof | 13% | 37 | 43 |
| built-ins/Boolean | 13% | 36 | 51 |
| language/expressions/addition | 12% | 36 | 48 |
| language/directive-prologue | 33% | 35 | 62 |
| built-ins/FinalizationRegistry | 0% | 34 | 47 |
| language/expressions/new | 33% | 34 | 59 |
| built-ins/WeakSet | 42% | 33 | 85 |
| language/computed-property-names | 31% | 33 | 48 |
| language/expressions/delete | 37% | 33 | 69 |
| language/expressions/division | 13% | 33 | 45 |
| language/statements/using | 12% | 33 | 78 |
| language/expressions/array | 30% | 32 | 52 |
| language/expressions/logical-assignment | 16% | 32 | 78 |
| language/expressions/greater-than | 20% | 31 | 49 |
| language/expressions/async-arrow-function | 0% | 30 | 60 |
| language/comments | 26% | 29 | 52 |
| language/expressions/multiplication | 15% | 29 | 40 |
| language/statements/switch | 9% | 29 | 111 |
| language/expressions/call | — | 28 | ∞ |
| language/expressions/less-than-or-equal | 27% | 28 | 47 |
| language/expressions/modulus | 15% | 28 | 40 |
| language/expressions/subtraction | 13% | 28 | 38 |
| language/white-space | 49% | 28 | 67 |
| built-ins/TypedArrayConstructors | 1% | 27 | 738 |
| built-ins/encodeURI | 3% | 27 | 31 |
| built-ins/encodeURIComponent | 3% | 27 | 31 |
| language/expressions/less-than | 22% | 27 | 45 |
| built-ins/AsyncGeneratorPrototype | 25% | 25 | 48 |
| language/expressions/equals | 31% | 24 | 47 |
| language/expressions/greater-than-or-equal | 30% | 24 | 43 |
| language/expressions/in | 13% | 22 | 36 |
| language/expressions/postfix-decrement | 10% | 22 | 37 |
| language/expressions/postfix-increment | 10% | 22 | 38 |
| language/expressions/prefix-decrement | 11% | 22 | 34 |
| language/expressions/prefix-increment | 12% | 22 | 33 |
| built-ins/Symbol | 20% | 21 | 98 |
| language/expressions/tagged-template | 0% | 21 | 27 |
| language/expressions/template-literal | 35% | 21 | 57 |
| language/expressions/unsigned-right-shift | 42% | 20 | 45 |
| built-ins/WeakRef | 0% | 19 | 29 |
| language/expressions/await | 0% | 19 | 22 |
| language/expressions/does-not-equals | 28% | 19 | 38 |
| language/expressions/left-shift | 44% | 19 | 45 |
| language/expressions/async-function | — | 18 | ∞ |
| built-ins/Set | 37% | 17 | 383 |
| language/block-scope | 18% | 16 | 145 |
| language/expressions/bitwise-and | 26% | 16 | 30 |
| language/expressions/bitwise-or | 26% | 16 | 30 |
| language/expressions/bitwise-xor | 26% | 16 | 30 |
| language/expressions/right-shift | 40% | 16 | 37 |
| language/global-code | 11% | 16 | 42 |
| language/line-terminators | 21% | 15 | 41 |
| language/statements/async-function | — | 15 | ∞ |
| built-ins/AggregateError | 4% | 13 | 25 |
| built-ins/RegExpStringIteratorPrototype | 0% | 13 | 17 |
| built-ins/parseFloat | 74% | 13 | 54 |
| language/expressions/property-accessors | 33% | 13 | 21 |
| language/expressions/unary-plus | 17% | 13 | 17 |
| built-ins/AsyncGeneratorFunction | 0% | 12 | 23 |
| built-ins/GeneratorFunction | 0% | 12 | 23 |
| language/expressions/new.target | 0% | 12 | 14 |
| language/statements/do-while | 16% | 12 | 36 |
| built-ins/SuppressedError | 4% | 11 | 22 |
| built-ins/ThrowTypeError | 0% | 11 | 14 |
| built-ins/parseInt | 78% | 11 | 55 |
| language/asi | 54% | 11 | 102 |
| language/expressions/strict-equals | 36% | 11 | 30 |
| language/expressions/logical-or | 38% | 10 | 18 |
| language/expressions/strict-does-not-equals | 40% | 10 | 30 |
| language/expressions/unary-minus | 14% | 10 | 14 |
| language/statements/while | 28% | 10 | 38 |
| built-ins/AsyncFunction | 11% | 9 | 18 |
| language/expressions/conditional | 40% | 9 | 22 |
| language/statements/labeled | 0% | 9 | 24 |
| built-ins/isFinite | 40% | 8 | 15 |
| language/expressions/logical-and | 50% | 8 | 18 |
| language/rest-parameters | 18% | 8 | 11 |
| language/statements/break | 5% | 8 | 20 |
| built-ins/AsyncIteratorPrototype | 0% | 7 | 13 |
| built-ins/Error | 34% | 7 | 93 |
| built-ins/isNaN | 40% | 7 | 15 |
| language/destructuring | 63% | 7 | 19 |
| language/expressions/optional-chaining | — | 7 | ∞ |
| language/expressions/typeof | 50% | 7 | 16 |
| language/future-reserved-words | 40% | 7 | 55 |
| language/identifier-resolution | 35% | 7 | 14 |
| language/statements/if | 28% | 7 | 69 |
| built-ins/MapIteratorPrototype | 18% | 6 | 11 |
| built-ins/NativeErrors | 40% | 6 | 94 |
| built-ins/SetIteratorPrototype | 18% | 6 | 11 |
| language/expressions/bitwise-not | 50% | 6 | 16 |
| language/expressions/logical-not | 63% | 6 | 19 |
| language/expressions/void | 33% | 6 | 9 |
| built-ins/Promise | — | 5 | ∞ |
| language/expressions/import.meta | 0% | 5 | 23 |
| built-ins/Infinity | 16% | 4 | 6 |
| built-ins/eval | 30% | 4 | 10 |
| language/expressions/comma | 16% | 4 | 6 |
| language/expressions/grouping | 55% | 4 | 9 |
| language/expressions/this | 16% | 4 | 6 |
| built-ins/ArrayIteratorPrototype | 70% | 3 | 27 |
| built-ins/NaN | 33% | 3 | 6 |
| built-ins/undefined | 37% | 3 | 8 |
| language/expressions/exponentiation | 61% | 3 | 44 |
| language/statements/block | 9% | 3 | 21 |
| built-ins/StringIteratorPrototype | 28% | 2 | 7 |
| language/expressions/coalesce | 66% | 2 | 24 |
| language/statements/expression | 0% | 2 | 3 |
| language/statements/throw | 85% | 2 | 14 |
| built-ins/BigInt | 0% | 1 | 77 |
| built-ins/global | 86% | 1 | 29 |
| language/expressions/assignmenttargettype | 2% | 1 | 324 |
| language/expressions/concatenation | 80% | 1 | 5 |
| language/expressions/member-expression | 0% | 1 | 1 |
| language/statements/empty | 50% | 1 | 2 |
| language/statements/for | — | 1 | ∞ |
| language/statements/try | — | 1 | ∞ |
| built-ins/AbstractModuleSource | 0% | 0 | 8 |
| language/expressions/relational | 100% | 0 | 1 |
| language/expressions/tco-pos.js | 0% | 0 | 1 |
| language/keywords | 0% | 0 | 25 |
| language/punctuators | 9% | 0 | 11 |
| language/source-text | 100% | 0 | 1 |
| language/statements/continue | — | 0 | ∞ |
| language/statements/debugger | 50% | 0 | 2 |
| language/statements/return | 31% | 0 | 16 |
