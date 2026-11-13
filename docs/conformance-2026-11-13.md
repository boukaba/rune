# Test262 targeted baseline — 2026-11-13

Command:

```sh
./target/debug/rune test262 built-ins/Promise
```

Result:

```text
Results: 353 passed, 285 failed, 65 skipped, 703 total
```

This is **50.2% of the suite** and **55.3% of executed tests** (638). It is a
targeted baseline, not the core-suite 90% gate from the production roadmap.

Observed failure categories in the tail of this run:

- harness/`$262` and executor-prototype tests reported as parse/runtime errors;
- `Promise.resolve`-function/subclass/species cases;
- non-constructor resolve/reject functions;
- `Symbol.species` return-value/name cases;
- async/iterator harness helpers not available in the standalone runner.

The suite includes `Promise.any` tests; the implementation landed in this round,
but the numbers above were measured after that build. Future runs should be
stored alongside the commit SHA and compared suite-by-suite rather than as one
aggregate.
