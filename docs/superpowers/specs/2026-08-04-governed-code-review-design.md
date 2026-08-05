# Governed code review: git substrate, derived graph, and the freehold review surface

Date: 2026-08-04
Status: approved design, pre-implementation

## Goal

Implement the spec's governed code-review scenario (Appendix F) end to
end, with allod's own repository as the first governed repo and freehold
as the review surface. The v1 gate is advisory: a non-required GitHub
Actions check that passes or fails against policy. Enforcement (§4.5
L2-enforced) and attested execution (§5.5) are out of scope for v1.

Decisions fixed during design:

- Dogfood target: the allod repository governs itself.
- Gate mode: advisory, delivered as a non-required CI check.
- Indexer depth: file-granular for all languages, function-level for
  Rust.
- Freehold role: full review surface (Inbox shows git proposals,
  decisions happen there).
- Review artifacts (§4.4) are in v1 scope.
- Approach: substrate trait refactor first (§3.1 abstraction), then the
  git binding and pipeline on top of it.
- GitHub comment sync: one-way ingest in v1, designed so the post-back
  direction can be added without rework.

## Architecture

Two layers, per §3.4: git stays authoritative for code, and a native
allod graph beside it (the governance graph) holds the derived code
graph, classifications, review artifacts, and the decision records that
govern git changesets. The governance graph lives in the allod repo at
`.allod/`, committed, so decisions travel by ordinary git push.

A consequence to state plainly: governance-graph updates are themselves
commits to the repo the graph governs. In advisory mode the loop is
harmless; a path rule scoped to `.allod/**` keeps those commits
admissible without review so the loop terminates.

### Capability split

1. Evaluation (Rust CLI, runs in CI). Deterministic operation sets,
   policy checklists, verdicts. Authoritative.
2. Governance data (wasm, native graph). Proposals, review artifacts,
   decision records. What freehold reads and writes. The git substrate
   is never compiled to wasm; freehold does not touch git objects.
3. GitHub connector (freehold, TypeScript). Watches remote git state,
   surfaces proposals in the Inbox, ingests PR comments, fetches diff
   content via the GitHub API for display.

### Substrate abstraction

New crate `allod-substrate` defines the §3.1 interface:

```rust
trait Substrate {
    fn revision(&self, hash: &RevHash) -> Result<Revision>;      // parents, author, signature, timestamp
    fn operation_set(&self, rev: &RevHash) -> Result<Vec<Operation>>; // deterministic
    fn state_hash(&self, rev: &RevHash) -> Result<StateHash>;
    fn heads(&self) -> Result<Vec<(RefName, RevHash)>>;
    fn verify_authorship(&self, rev: &RevHash) -> Result<AuthorVerdict>;
}
```

Implementations:

- `NativeSubstrate`: adapter over the existing changeset log
  (`allod-core` store/fold, `allod-graph` plumbing). This is a
  re-homing, not a rewrite. Zero behavior change; existing tests pass
  unmodified.
- `GitSubstrate` (crate `allod-substrate-git`, gix-based): commit =
  changeset, operation set = byte-level tree diff against the first
  parent with rename detection disabled (§3.3), tree hash = state hash,
  refs = heads, commit signature = authorship. Built over a pluggable
  object source so CI checkouts and maintained mirrors both work.
  Unresolvable remote objects report degraded evidence, never hard
  failure (§3.4 rule 2).

### Policy

`allod-core/src/policy.rs` already parses `substrate: git` selectors and
evaluates `reviewers: {role, quorum}` requirements with multi-approval
counting. The work is to unstub git-selector matching (repo, path, and
ref patterns against substrate operation sets) and to implement region
reach: a region rule matches a git changeset when the commit's operation
set touches a path from which an in-region derived object derives, in
the derived graph as of the parent revision (§8.3). No symbol- or
span-identity matching across commits.

### Indexer

Crate `allod-index-code`, implementing §8.3:

- File-granular for all languages: Repository and SourceFile nodes with
  `git:` external refs. Function-level for Rust via syn: Function/Type
  nodes and declares edges. No source text in the graph.
- Commit-aligned derivation: one derived changeset per imported commit,
  so the graph is materializable as of any commit.
- Full lineage on every object: `derived_from` input hashes,
  `derived_by` indexer principal, `method: deterministic`, `tool` with
  versions. Idempotent: same commit + same tool version yields equal
  changesets.

### CLI and CI action

- `allod git eval <commit> --target <ref>`: computes the operation set,
  matches policy, reports the checklist (required roles/quorums, which
  are satisfied by decision records present in the graph) and a verdict,
  as JSON.
- `allod git index <commit>`: runs derivation up to the given commit.
- A GitHub Actions workflow runs both on `pull_request` and `push`,
  posting the verdict as a non-required check run with the checklist in
  the summary. Failing is the designed steady state early on; the
  summary must make "what is unmet and where to decide it" legible.

In CI, derived changesets are recomputed on demand and not pushed (the
workflow needs no push rights). They are materialized into `.allod/`
when a human or freehold session runs the indexer and commits. Revisit
if recompute cost grows.

### Review ontology

New package `review`, drafted in milestone 2 so data shapes freeze
before freehold builds against them:

- `Review`: verdict state (`approve | approve-with-comments |
  request-changes`), body. Feeds a decision record's `basis` (§4.4).
- `ReviewComment`: body, anchor (`git:` ref + span), thread parent,
  status, and external-provenance fields for GitHub-ingested comments.
- Edges: review → ChangeRequest/commit target, comment → review,
  comment → derived code objects it concerns.

Exact attribute lists are settled during milestone 2 implementation
against §4.4's structure requirements.

### Freehold surface

- wasm additions to `@allod/core`: list/read git-substrate proposals,
  write review artifacts, write decision records referencing commit SHAs
  by content hash.
- Inbox: git proposals listed beside native ones, showing checklist
  state, the semantic diff from the derived graph, and both file
  versions fetched via the GitHub API. Deciding writes the signed
  decision record into `.allod/`; once pushed, the next eval run turns
  the check green.

### GitHub connector

One event-handling core behind two orthogonal choices:

- Auth: **credential mode** (token discovered from `gh auth token`,
  falling back to the git credential helper; no registration) or
  **GitHub App mode** (manifest-flow wizard: freehold builds the
  manifest server-side, signs state with an HMAC, the browser form-posts
  to GitHub's app-creation page, and the setup callback exchanges the
  temporary code via `/app-manifests/{code}/conversions` and stores
  credentials — PEM, webhook secret, client secret encrypted; app
  id/slug as metadata. Installation tokens minted with an app JWT and
  cached with an expiry margin). App mode works locally too; the wizard
  gates the webhook toggle on whether a public URL is configured.
- Transport: webhooks (`push`, `pull_request`, `pull_request_review`,
  `issue_comment`; `X-Hub-Signature-256` validated) when a public URL
  exists — the multi-user cloud deployment — otherwise polling. Both
  transports feed the same handlers; a startup catch-up poll covers
  missed deliveries.

The pattern is ported from valet's dev-v2 branch
(`packages/api/src/routes/github-app.ts`,
`packages/web/src/components/settings/github-app-section.tsx`): same
stack, no external GitHub libraries. Connector state (app credentials,
installation table, comment-ID mapping) lives in freehold's PGlite
database.

Comment ingest is one-way in v1: GitHub PR comments and reviews become
`ReviewComment` nodes with external provenance. The comment-ID mapping
table is the seed for later bidirectional sync.

## Milestones

Each independently shippable, in order:

1. **Substrate refactor.** `allod-substrate` trait, `NativeSubstrate`
   adapter, fold/policy consuming the trait. Zero behavior change.
2. **Git evaluation + CI action.** `allod-substrate-git`, policy
   unstub, `allod git eval`, advisory check on allod's own PRs with
   path/ref rules. `review` ontology package drafted.
3. **Derived graph.** `allod-index-code`, commit-aligned derivation,
   region-reach rules live.
4. **Freehold review surface.** wasm additions, connector (credential
   mode first, App wizard second), Inbox for git proposals, decision
   records closing the loop.

Milestone 4 is where Appendix F runs end to end: push a branch → CI
evaluates and holds → Inbox shows the semantic diff → decide → decision
record lands in `.allod/` → check goes green.

## Error handling and edge cases

- **Force-push / rebase.** Decision records bind to commit SHAs; a
  rebase orphans them by construction and the new head is undecided.
  Correct behavior, not an error. The Inbox shows the proposal as
  needing re-decision and links the superseded record. No heuristic
  carry-forward.
- **Unreachable remote objects.** Reported as degraded, never failure.
  The CI check distinguishes policy-unmet (fail) from evidence-offline
  (pass, annotated).
- **Schema-context mismatch.** Derived changesets record the
  meta-subgraph hash they validated against (§3.2.1); eval reports a
  mismatch explicitly rather than re-validating silently.
- **Comment ingest.** Idempotent by GitHub comment ID; edits update the
  node, deletions tombstone it. GitHub actors without a principal
  binding are recorded as claimed identities, visible as unsigned
  provenance, never silently promoted.
- **Concurrent graph writes.** Freehold decisions and indexer
  materializations both commit to `.allod/`; update operations carry
  prior revision hashes (§3.2.2), so conflicts surface as explicit
  merge resolution, never last-writer-wins.

## Testing

- **Trait conformance suite** in `allod-substrate`: §3.1 property tests
  run against both substrates. The refactor milestone passes when native
  conformance and all existing tests are green.
- **Determinism vectors** for git operation sets: a fixture repo with
  renames, mode changes, binary files, merge commits, and empty commits;
  byte-identical operation sets across runs. These become spec test
  vectors in the spirit of Appendix H.
- **Policy fixtures:** git-selector matching, region reach at parent
  state, quorum satisfaction from decision records.
- **Indexer idempotence:** same commit + tool version → equal derived
  changesets.
- **End-to-end fixture:** a scripted repo where a branch touches a
  `security/critical`-classified file; held → decide → green. Runs in
  allod CI via the CLI and in freehold CI via wasm + connector with a
  mocked GitHub API.
- **Connector:** webhook signature validation, poll/webhook parity
  (identical graph writes through both transports), ingest dedup under
  redelivery.

## Out of scope for v1

- Enforcement (ref gate, merge queue, pre-receive hook) and the attested
  gate (§5.5).
- Bidirectional comment sync (posting graph reviews back to PRs).
- Function-level indexing beyond Rust; SCIP/LSIF-based extraction.
- Webhook delivery for local deployments (polling covers it).
