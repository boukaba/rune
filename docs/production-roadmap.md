# Rune Production-Readiness Roadmap

**Status:** Phase 0 (reset) — started 2026-11-13
**Baseline reviewed:** commit `5d940ae` — see [the deep review](review-2026-11-12.md)
**Goal:** make Rune the default JavaScript engine for Rust/edge infrastructure:
sub-millisecond application snapshots, <10 MB per isolate, capability-secure,
Web-standards compatible, and honest about conformance.

This roadmap is deliberately not a race with V8 on peak throughput. Rune wins
on cold start, memory, embedding, security, and deterministic cached
compilation. It must first stop producing wrong answers.

---

## North-star metrics

| Metric | Target |
|---|---|
| Cold start with application snapshot | < 1 ms p50, < 3 ms p99 |
| Idle RSS per isolate | < 10 MB |
| Core test262 (per-suite median) | 95%+ |
| Differential corpus vs Node | 100% of supported programs |
| Fuzzing | 10,000+ CPU-hours clean before 1.0 |
| GC pauses | < 1 ms p99 for small heaps |
| Production design partners | 3 by 0.9, 10+ by 1.0 |
| Contributors | 20–40 regular, 200+ lifetime |

---

## Phase 0 — Reset (0–2 months)

**Rule: no new language features.**

1. Finish the P0 soundness fixes: GC roots, self-moving growth, continuation
   stacks, JIT opt-in, async/ESM/semantics blockers.
2. Make `cargo test --workspace` work offline and in CI without research
   dependencies (done: spikes moved to `spikes/`).
3. Add a differential corpus (Node oracle), GC-stress mode, and JIT-on/off
   comparison to CI.
4. Split the interpreter monolith into contributor-sized crates/modules.
5. Publish `CONTRIBUTING.md`, `ARCHITECTURE.md`, `GOVERNANCE.md`,
   `SECURITY.md`, and a public issue taxonomy.
6. Seed 100 reproducible “good first issue” tasks from test262 failures,
   runtime adapters, and tooling.

**Exit gate:** no wrong answers or panics in the semantic corpus; JIT disabled
by default; GC-stress CI green; 100 seeded issues; three design partners in
conversation.

---

## Phase 1 — Engine hardening (2–6 months)

- **Memory:** stable object/array headers with separate slot storage;
  generational nursery/old space; write barriers; incremental marking; weak
  refs/finalization; bounded shape arena. Evaluate MMTk before building a
  full collector.
- **Continuations:** one stack abstraction for every pending machine; remove
  global depth rebasing; callee+depth identity.
- **JIT:** keep runtime JIT experimental; invest in build-time AOT (Cranelift)
  with guards, not a TurboFan-class runtime compiler.
- **Semantics:** async/await continuations, ESM live bindings, `finally` +
  `return`, function hoisting, arrow lexical `this`, captured updates, TDZ.
- **Conformance:** public dashboard, spec pods, builtin owners.
- **Security:** capability model, fuel/memory/time limits, deterministic
  time/random, W^X/PAC, cache signing.
- **Fuzzing:** grammar-based JS fuzzer plus differential fuzzing.

**Exit gate:** 90–95% core test262; 1,000 fuzz hours; zero P0/P1; debugger and
profiler proof of concept.

---

## Phase 2 — Runtime and build tool (6–12 months)

- Web APIs: `fetch`, `Request`/`Response`, `Headers`, `URL`, streams,
  `TextEncoder`, `AbortController`, `crypto`, `EventTarget`, timers.
- HTTP server/client, TLS, WebSocket, environment/secrets, structured logs,
  OpenTelemetry.
- Module loader: ESM, import maps, URL imports, lockfile; npm resolver and a
  Node-builtin subset later.
- `rune build`: AOT native code + application snapshot + bytecode; small,
  reproducible, cross-compiled standalone binaries.
- Bindings: stable C ABI, Python, Go, WASM; Rust embedding with capabilities
  and async host functions.
- Lighthouse demos: Hono on Rune, Axum embedding, Lambda adapter, multi-tenant
  plugin sandbox, edge cache.

**Exit gate:** Hono runs; snapshot boot < 1 ms; three production design
partners; 0.9 release.

---

## Phase 3 — 1.0 and production assurances (12–18 months)

- Security audit, threat model, CVE process, bug bounty, SBOM, signed
  releases, reproducible builds.
- DAP debugger, CPU profiler, heap snapshots, stack traces/source maps,
  `rune inspect`.
- Performance budgets and public benchmarks: cold start, RSS, p99 latency,
  throughput, binary size, GC pauses.
- API stability, semver, deprecation policy, LTS.
- npm subset for pure-ESM packages; explicit compatibility matrix.
- Support/SLAs through a company or foundation.

**Exit gate:** 1.0; 10+ production users; 100+ lifetime contributors; 10,000
fuzz hours; external audit complete.

---

## Phase 4 — Category leadership (18–24 months)

- Foundation/governance, SIGs, RFC process, funded maintainers.
- Certifications, conferences, university partnerships, GSoC/Outreachy.
- Ecosystem: package loading, deployment adapters, observability integrations.
- Target: recognized default for JavaScript inside Rust/edge infrastructure;
  50+ production users; hundreds of lifetime contributors.

---

## Non-goals

- Full Node.js or browser compatibility.
- Peak-throughput parity with V8/JSC/SpiderMonkey.
- A TurboFan-class runtime optimizing JIT.
- Chasing test262 coverage before correctness under GC and JIT.

---

## Working agreement

- Every phase/step ships with: tests, docs sync (`progress.md`, `README.md`),
  `cargo fmt`, clippy with CI flags, and a commit pushed as `boukaba`.
- A feature is not “done” until it has a differential or conformance test and
  works under GC stress with JIT on and off.
- Performance claims require a reproducible benchmark and a correctness
  assertion.
