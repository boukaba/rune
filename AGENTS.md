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
- **Known gaps**: No thenable unwrapping, `async_reject` is_throw path not yet wired to generator throw, `.finally` pending case falls back to old .then behavior, RegExp: no function replacement, no match/search/split, prototype properties missing (source/flags/lastIndex), `class`: no `extends`, no `static` methods, no computed method names, `this.prop++` not supported (Update only handles Identifier targets).

### Next Steps — v0.5 (ordered by leverage)
1. Thenable unwrapping for `Promise.resolve`
2. RegExp prototype properties (source/flags/lastIndex), function replacement for replace
3. `class` `extends` support
