# Test262 targeted baseline — 2026-11-13

Command:

```sh
./target/debug/rune test262 built-ins/Promise
```

Result:

```text
Results: 371 passed, 267 failed, 65 skipped, 703 total
```

This is **52.8% of the suite** and **58.2% of executed tests** (638). It is a
targeted baseline, not the core-suite 90% gate from the production roadmap.

Observed failure categories in the tail of this run:

- harness/`$262` and executor-prototype tests reported as parse/runtime errors;
- `Promise.resolve`-function/subclass/species cases;
- non-constructor resolve/reject functions;
- `Symbol.species` return-value/name cases;
- async/iterator harness helpers not available in the standalone runner.

Follow-up fixes (prototype-chain dispatch for `instanceof`/`Object.getPrototypeOf`,
String-wrapper prototypes, `Array.prototype.toString`, array-like String/function
receivers, wrappers, AggregateError) moved the suite to 375; then correct
undeclared-identifier semantics plus a real `$DONE` shim exposed four
wrong-reason passes, so the honest baseline is **371**. The targeted
`built-ins/Array/prototype/map` suite is 199/216 (92.1%).

The suite includes `Promise.any` and combinator-thenable tests; the numbers
above were measured after those implementations landed (commit series ending
with the thenable-combinator change). Future runs should be
stored alongside the commit SHA and compared suite-by-suite rather than as one
aggregate.
