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

Decisions are stored as git notes under `refs/notes/allod-decisions`.

Push: `git push origin refs/notes/allod-decisions`

Fetch: `git fetch origin refs/notes/allod-decisions:refs/notes/allod-decisions`

## Keys

`.allod/keys/` is gitignored. The signing key exists only on the owner's machine. CI verifies proposals and decisions using the public keys recorded in the graph state.
