# Rune security model (2026-11-13)

Status: **partial**. This document records what the engine enforces today, what
it explicitly does not enforce, and the remaining production gates. It is not a
claim that Rune is a sandbox.

## Threat model

Rune executes JavaScript supplied by an application or tenant. The engine is
written in Rust, but uses `unsafe` for the GC heap, NaN-boxed values, JIT code
pages, and property-slot storage. Memory-safety bugs in those paths are
security bugs; the GC-stress, parser-fuzz, and JIT-differential suites are
regression guards, not a proof.

The engine does **not** provide OS isolation. Untrusted tenants should run in a
separate process/container with filesystem/network limits; in-process Rune is
not a security boundary.

## Enforced today

- **Deterministic instruction budget.** `Context::set_instruction_budget(n)` and
  `rune --max-instructions=N` bound interpreted work. Exhaustion raises a
  catchable `RangeError` once, then resets to unlimited so the handler can run.
  While a budget is active the JIT is suppressed, so native code cannot bypass
  the counter.
- **Wall-clock budget.** `Context::set_time_budget(duration)` and
  `rune --timeout-ms=N` check a deadline every 4096 interpreted instructions.
  Expiry raises a catchable RangeError once and clears the deadline. The JIT is
  equally suppressed while a deadline is active.
- **Network policy.** `FetchPolicy` defaults to http/https with literal
  loopback/private/link-local/CGNAT/ULA hosts blocked. Exact/`*.suffix`
  allowlists and `allow_private_networks` are opt-in. Host blocking happens
  before the transport and rejects the returned Promise.
- **Bundle integrity.** `RUNEAPP2` bundles embed a source hash plus an
  engine/ABI tag; stale or foreign bundles are rejected rather than executed.
- **GC stress and fuzzing.** CI runs `RUNE_GC_STRESS`, a deterministic
  token-soup parser corpus, and JIT-on/off differential tests.
- **Offline builds.** The workspace builds/tests with a workspace-local
  `CARGO_HOME` and `--offline`.

## Not enforced / known gaps

- **No per-script heap cap.** The semispace is fixed per `Context`;
  allocation exhaustion is an engine-level failure.
- **Budgets are execution-only.** Parsing/compilation, host functions, event-loop
  jobs, and blocking I/O are not covered by the instruction/time budgets. The
  time budget observes the clock only between interpreter instructions.
- **DNS rebinding / redirects / proxy / TLS pinning.** Fetch policy checks the
  literal host only. A transport may resolve a public name to a private address,
  follow redirects to blocked hosts, or use system proxy settings. No
  certificate pinning or custom CA policy.
- **No syscall/filesystem/network sandbox.** Host functions are ambient
  capabilities; embedders must not expose privileged closures to untrusted
  code. Host closure captures are not GC-rooted by the engine.
- **No bundle signing/encryption.** Integrity is hash/ABI-scoped, not
  authenticity-scoped.
- **JIT proof.** Branch-containing traces are gated off; the remaining native
  tier is differential-tested but not formally verified. JIT is automatically
  disabled while a budget is active, but otherwise JS-visible native bugs remain
  a risk.
- **Resource exhaustion through recursion, strings, regexes, or TypedArrays**
  is not uniformly bounded; individual operations may allocate large objects.

## Production gates

1. Per-execution wall-clock and heap limits that also cover native code.
2. Resolve-and-check redirect/DNS policy with an explicit transport contract.
3. Capability-based host bindings (no ambient privileged globals).
4. Reproducible, signed application bundles.
5. Audit of every `unsafe` GC/JIT/property path plus continuous fuzzing across
   parser, GC, API, and native tier.
