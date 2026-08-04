# Allod library refactor + WASM bindings — design

**Date:** 2026-08-03
**Status:** Approved, pending implementation plan
**Scope:** Promote the functionality trapped in the `allod` CLI binary into
library crates behind a storage abstraction, and publish a WASM npm package
so TypeScript clients (first: Freehold) can operate graphs without shelling
out. The CLI becomes a thin shell. On-disk format, CLI behavior, test
vectors, and the mvp.rs acceptance test are all preserved.

## Context

`allod-core` is already a clean domain layer (canon, hash, sign, model,
fold, policy, store, vocab, registry, typeexpr). But everything a client
actually wants sits in the binary crate: `main.rs` holds the command flows
(init, principal-add, note, propose, decide, classify, checkpoint, verify,
proposals, log, show) as `cmd_*` functions that print to stdout and return
`Result<_, String>`, and `md.rs` (markdown bundle), `repo.rs` (repo import,
semantic diff), and `fed.rs` (peers, grants, bundles, imports) are binary
modules — roughly 2,500 lines of real functionality no client can link
against.

Freehold (see `~/code/freehold/docs/specs/2026-08-03-freehold-v0-design.md`)
is the first consumer: a TypeScript daemon that needs the full memory flow
(genesis → agent → note → propose → approve → verify) in-process, in Node
today and potentially in the browser later.

## Decisions (locked)

1. **Crate layout.**
   - `allod-core` — the pure domain. Loses direct filesystem access:
     storage moves behind a `LogStore` trait. No other semantic changes.
   - `allod-graph` (new) — the operations API. A `Graph` handle owning a
     `Box<dyn LogStore>`, with two layers: **generic operations** —
     atomic changeset building over arbitrary installed types (create,
     update, delete of nodes, edges, classifications, and documents),
     propose/decide, registry introspection (entity types with attribute
     schemas and inheritance, edge types with domain/range, the taxonomy
     DAG), and schema-document install — and the **current command
     flows** (init, note, propose-preference, verify, checkpoint, and
     the rest) reimplemented on top of them, plus the markdown bundle,
     repo import + semantic diff, and federation. Clients like Freehold
     build on the generic layer; the CLI and demos use the flows.
     Methods return typed values (serde-serializable structs); nothing in
     the library prints.
   - `allod` — the CLI, thinned to argument parsing, calls into
     `allod-graph`, and output formatting. The demos and `tests/mvp.rs`
     stay here.
   - `allod-wasm` (new) — `wasm-bindgen` wrapper over `allod-graph`,
     published to npm as `@allod/core`.
   - `allod-lint` and `allod-vectors` — unchanged in behavior; they may
     migrate to `allod-graph` APIs where that removes duplication.

2. **`LogStore` is a synchronous trait.** The trait covers what `store.rs`
   does today: read/write changesets, materialized objects, checkpoints,
   principals and keys, and schema packages. Two implementations:
   - `FsStore` (native) — the current on-disk layout, byte-for-byte
     unchanged. Existing graphs open without migration.
   - `MemStore` (everywhere, and the WASM default) — in-memory, with
     explicit `load`/`dump` of the same serialized forms.
   Keeping the trait sync avoids threading async through the fold. The
   WASM wrapper bridges to asynchronous JS persistence at its own layer
   (decision 4).

3. **Structured errors.** `Result<_, String>` becomes a `thiserror` enum
   in the library: `SchemaViolation`, `PolicyRejected`, `HashMismatch`,
   `SignatureInvalid`, `UnknownPrincipal`, `NotFound`, `Storage`, and
   related variants. A write that policy holds for review is **not an
   error**: admission returns `Admitted(changeset)` or `Held(proposal)` as
   `Ok` variants, because a hold is the system working as intended. The
   CLI formats these; clients match on them.

4. **WASM persistence contract.** `@allod/core` wraps `MemStore`. The JS
   host supplies persistence callbacks (async is fine); the wrapper awaits
   them via `wasm-bindgen-futures` after each mutating operation, before
   the operation resolves. The serialized form written by those callbacks
   is the **same file layout `FsStore` reads**, so a graph written from
   TypeScript opens in the Rust CLI and vice versa. The package ships a
   ready-made Node/Bun filesystem backend; a browser OPFS backend is
   possible later without touching Rust.

5. **Native-only features stay native.** Repo import shells out to git,
   so `allod-graph` gates it (and anything else process-spawning) behind a
   `native` cargo feature excluded from the WASM build. The WASM surface
   for v1 of the package is the generic layer plus the flows built on it:
   graph lifecycle, principals, generic object operations over arbitrary
   installed types, proposals and decisions, checkpoints, verification,
   registry introspection, schema-document install, the memory flows as
   reference sugar, markdown bundle export/import, and federation
   bundle/import (pure data, no network). The package also bundles the
   reference ontology packages (`core`, `memory`, and their policies) as
   data, so clients need no copy of the allod repo; additional packages
   install from caller-supplied documents.

6. **No behavior changes.** This is a refactor with one new delivery
   vehicle. The Appendix H vectors, `allod-lint` output, CLI output that
   tests depend on, and the on-disk format are all invariant. Anything
   that would change behavior is out of scope and waits.

## Exit criteria

1. `cargo test` across the workspace passes, including `tests/mvp.rs`,
   with the vectors regenerating byte-identical.
2. The CLI runs the three demos with unchanged behavior.
3. A TypeScript test in `allod-wasm` (run under Node in CI) executes the
   full memory flow through `@allod/core`: genesis with a root keypair,
   agent registration, a scratch note admitted immediately, a preference
   proposal held, the owner's approval admitting it, and three-level
   verification passing.
4. Cross-implementation interop: a graph created by that TypeScript test
   is opened and verified by the Rust CLI (`allod verify`), and a graph
   created by the CLI is opened and verified from TypeScript.

## Testing

- Existing suite (mvp.rs, vectors, lint self-checks) runs unchanged —
  the primary regression net.
- `LogStore` conformance suite run against both `FsStore` and `MemStore`.
- The WASM package gets its own vitest suite covering the memory flow,
  the persistence contract (a killed process resumes from persisted
  state), and the interop exit criterion.
- Error-path tests at the `allod-graph` layer: held vs rejected vs
  schema-invalid are distinct, typed outcomes.
