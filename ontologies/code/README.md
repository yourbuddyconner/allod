# code: the derived code-graph ontology

The ontology that indexers emit into (§8.3). A repo import walks a
repository at a commit and proposes nodes for files, modules,
functions, and types, with edges for imports, calls, references, and
implementations. Everything under this ontology is derived: lineage
`method` is `deterministic` for SCIP, LSIF, or tree-sitter extraction,
and `model-assisted` extraction faces stricter admission (§8.2).

Three rules from §8.3 shape the package:

- **The graph holds hashes.** Nodes reference blobs and commits
  through `git:` external refs, and the graph describes the code
  without containing it.
- **Derivation is commit-aligned.** Each imported commit yields one
  derived changeset, so the code graph is materializable as of any
  commit, and comparing two commits yields a semantic diff.
- **Packaging is separate.** Derived knowledge regenerates safely, and
  asserted knowledge is a human or agent claim (§8.4). Keeping the
  code ontology separate from `corp` and `eng` keeps that line sharp.

Tree-sitter extraction is a conforming fallback at lower resolution:
it fills `SourceFile` and `declares` reliably and leaves `calls` and
`references` sparse.

## Status

Draft, tracks spec v0.3. Import hashes are placeholders until the
reference implementation generates real state hashes (Appendix H).
