# Part 8 — Indexing Contract *(L0 pipeline / L3 attestation)*

## 8.1 The pipeline

Indexing turns unstructured sources into governed knowledge:

```
document(s) → [indexer] → classification + structure → changeset PROPOSAL → admission (Part 4)
```

The contract has four rules:

1. An indexer consumes documents (by content hash) and a schema version.
   It emits **proposals**. It never emits directly-admitted state. The
   indexer is a principal (§6.3). Its proposals face policy like anyone
   else's (§4.3).
2. Every emitted object carries lineage: `derived_from` the input
   document hashes, `derived_by` the indexer principal, plus `method`
   and `tool` (§5.1).
3. Indexing is idempotent over inputs. The same documents, schema, and
   indexer version SHOULD yield an equivalent proposal. Deterministic
   indexers yield equal proposals. Model-assisted indexers yield
   semantically equivalent ones (§8.2).
4. An indexer declares its **scope**: the taxonomy regions and entity
   types it may propose into. Policy MAY hold it to that scope
   (§4.2 `authors`).

## 8.2 Deterministic vs. model-assisted classification

| Basis | Definition | Trust lever |
|---|---|---|
| `deterministic` | Output is a pure function of inputs plus tool version. Examples: parsers, compilers, LSIF/SCIP emitters | Reproducibility. Anyone can re-run and compare |
| `model-assisted` | An ML model participates. Output may vary across runs and versions | Provenance plus attestation. You cannot re-derive it, so you must be able to verify what produced it |

Rules for model-assisted classification:

- `tool` MUST name the model identity as precisely as the deployment
  allows: model ID plus version, and the weights hash where available.
- Policy SHOULD route model-assisted proposals through stricter admission
  than deterministic ones. Appendix C does this.
- At L3, the attested-indexer envelope (§5.5) binds the input hashes, the
  indexer code identity, the model identity, and the output changeset
  hash. Machine-written knowledge becomes exactly as trustworthy as the
  measured pipeline. No more, no less, and verifiably so.

## 8.3 Derived graphs from external substrates (repo import)

This section applies the contract to a git substrate: **import a
repository as a knowledge graph about the code.**

Two layers. Never conflate them:

1. **The substrate binding (§3.3).** Commits are changesets. Git stays
   authoritative for content. Nothing is imported.
2. **The derived graph (this section).** An indexer walks the repo at a
   commit. It proposes nodes for files, modules, functions, and types.
   It proposes edges for imports, calls, references, definitions, and
   implementations. All of it lands under a **code ontology**, packaged
   separately from the core ontology.

Rules:

- **No source text in the graph.** Nodes reference blobs and commits by
  SHA, as `git:` external refs (§3.4). Both sides are content-addressed,
  so the linkage is exact. The graph is knowledge about the code. It is
  not a copy of the code.
- **Commit-aligned derivation.** Each imported commit yields one derived
  changeset. The knowledge graph's history mirrors the repo's history.
  The graph is materializable as of any commit.
- **Edge sources.** Deterministic extraction SHOULD use existing
  code-intelligence formats, mapped into the code ontology. SCIP is
  preferred. LSIF is acceptable. Allod adds governance, lineage,
  diffability, and unification with non-code knowledge. It does not
  re-specify code intelligence. Tree-sitter-grade syntactic extraction
  is a conforming fallback: `deterministic`, at lower resolution.
- **Toolchain honesty.** LSP and SCIP output depends on toolchain
  versions and configuration. This is why `tool` versioning is REQUIRED
  lineage, and why the attested indexer exists. When a code graph must
  be trusted, run the extraction at L3. Dependency review of enclave
  code is the motivating case.

What falls out: **semantic diff.** Compare the graph at commit A with the
graph at commit B. The result is a knowledge-level diff of the codebase:
which functions changed, what calls into them, the blast radius. The
graph computes it. Nobody guesses it. A review artifact (§4.4) can anchor
its sections to both the file hunks and the affected subgraph. A
reviewer, human or agent, gets the full-context, both-versions view that
diff-only tooling cannot provide.

## 8.4 Re-indexing and invalidation

- A schema version bump or an indexer upgrade MAY trigger re-indexing.
  Re-indexed output supersedes prior derived objects through ordinary
  `update` operations. Prior revisions stay in history with their
  lineage. You can always ask what rust-analyzer 0.3 thought this was.
- Derived objects MUST be marked as derived: their lineage `method` is
  not `manual`. Implementations can then distinguish re-derivable
  knowledge, which is safe to regenerate, from asserted knowledge, which
  is a human or agent claim. Regeneration must never silently overwrite
  an asserted claim. The no-last-writer-wins rule (§3.2.3) applies with
  full force.
- When an input document is redacted (§3.2.2), derived objects that cite
  it keep their tombstoned lineage. The claim survives, marked as
  resting on removed evidence. It stays visible. It is never silently
  orphaned.
