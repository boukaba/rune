# Test262 targeted baseline — 2026-11-13

Command:

```sh
./target/debug/rune test262 built-ins/Promise
```

Result:

```text
Results: 359 passed, 279 failed, 65 skipped, 703 total
```

This is **51.1% of the suite** and **56.3% of executed tests** (638). It is a
targeted baseline, not the core-suite 90% gate from the production roadmap.

Observed failure categories in the tail of this run:

- harness/`$262` and executor-prototype tests reported as parse/runtime errors;
- `Promise.resolve`-function/subclass/species cases;
- non-constructor resolve/reject functions;
- `Symbol.species` return-value/name cases;
- async/iterator harness helpers not available in the standalone runner.

Follow-up fixes (prototype-chain dispatch, wrappers, AggregateError, undeclared
identifiers) moved the suite upward; then the runner began actually draining
microtasks and honoring `$DONE(error)`. The honest async-completion baseline is
**359**, because assertions that were previously never reached now run. The
latest gains come from Promise-constructor semantics, generic `catch`/`finally`
dispatch, self-resolution rejection, and deferred thenable jobs. The targeted
`built-ins/Array/prototype/map` suite is 203/216 (94.0%). Adding
`.constructor` back-references to built-in prototypes lifted `String` from
454 to 523/1223 and `String.prototype.split` from ~11 to 78/120. Locale/string statics and
primitive prototype dispatch raised `String` to 592/1223 and made
`(5).toString()`/`true.toString()` reachable through their prototypes.

The suite includes `Promise.any` and combinator-thenable tests; the numbers
above were measured after those implementations landed (commit series ending
with the thenable-combinator change). Future runs should be
stored alongside the commit SHA and compared suite-by-suite rather than as one
aggregate.
