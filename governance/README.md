# governance/

This directory contains the allod repository's own review policy source.

## Policy

`policy.yaml` declares the projection-form policy evaluated by `allod git eval` in CI. Rules cover changes targeting `refs/heads/main`, workflow files on any branch, and changes to this directory itself. Each rule requires one decision from a `code-owner`.

## Genesis

The `.allod/` graph at the repository root was created by running:

```
bash scripts/init-governance.sh
```

That script calls `allod init`, `allod install-policy`, and `allod verify` once from the repo root. It is safe to read; running it again will refuse if `.allod/` already exists.

## Decisions

Decisions are stored in two forms depending on the target. Native graph changesets record their decision evidence in `.allod/changesets/<hash>.evidence.yaml` alongside the changeset; decisions about git commits travel as git notes under `refs/notes/allod-decisions` so CI can read them without rewriting the decided commit.

Push: `git push origin refs/notes/allod-decisions`

Fetch: `git fetch origin refs/notes/allod-decisions:refs/notes/allod-decisions`

## Keys

`.allod/keys/` is gitignored. The signing key exists only on the owner's machine. CI verifies proposals and decisions using the public keys recorded in the graph state.

## Derived graph

The `.allod/` graph contains a materialized derived graph of the repository's source tree. The owner materializes it by running:

```
cargo run -q -p allod -- install-schema . code --as conner --schema ontologies
cargo run -q -p allod -- git index . HEAD --as conner
```

The first command installs the `code` schema package (run once, on a graph initialized with the `memory` profile). The second command scans the commit tree and admits a derivation changeset covering every source file and extracted Rust item. Re-running on the same commit reports "up to date". The `.allod/` directory is excluded from indexing.

Classifying a node in the derived graph with a region term extends the policy's reach. The `security-critical-code` rule in `policy.yaml` fires on any commit that touches a path from which a `security/critical`-classified node derives. CI evaluates commits against the committed derived graph using `allod git eval`; the owner materializes and classifies — CI holds no signing key and does not write to the graph.
