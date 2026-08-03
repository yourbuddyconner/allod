# Part 7: Serialization Bindings (Projections) *(L0)*

All bindings in this Part are projections of the log (design
principle 1), and the log stays the only authoritative form. A projection
either round-trips to an identical state hash or declares itself lossy.

## 7.1 Canonical wire form (normative)

- **Encoding.** CBOR, per RFC 8949 Core Deterministic Encoding: definite
  lengths, sorted map keys, shortest-form integers. All hashes and
  signatures are computed over this encoding.
- **Textual twin.** JSON via JCS (RFC 8785), for debugging and for
  environments hostile to binary. The CBOR form is authoritative, and the
  JSON form MUST re-encode to byte-identical CBOR.
- **Envelopes.** Every wire object is tagged:
  `{ allod: <spec-version>, kind: <object-kind>, body: … }`.
  An implementation MUST reject an unknown top-level kind, and MUST
  preserve and re-emit unknown fields. This keeps minor spec versions
  forward-compatible.
- Log segments, checkpoints, proposals, decision records, and attestation
  envelopes all exchange in this form. There is no other normative
  interchange encoding.

## 7.2 Markdown bundle binding (normative, declared-lossy)

This is the human-facing projection: a directory of markdown files with
YAML front matter. It exists because adoption requires text that people
can casually read and edit. It is also the projection that agent
memory systems already use.

Layout and conventions:

- One file per node. The path derives from the taxonomy: the primary term
  maps to the directory.
- Front matter carries the envelope: `id`, `rev`, `type`,
  classifications, and key attributes. The prose body maps to the
  attribute the schema marks `long_text: true` (§2.1); trailing
  newlines are not significant in the body.
- Edges project as typed wiki-links inline (`[[node-id|label]]` with an
  edge-type annotation) and/or as a `links:` block in front matter.
- A redacted object (§1.8) projects as front matter only, marked
  `redacted: true`.
- A bundle-level `.allod/` directory carries the schema projection, the
  checkpoint reference, and the state-hash manifest.

Declared losses: placement of non-primary classifications, edge
attributes outside the annotated set, binary attributes, and history. A
bundle contains the current state only, without the log.

Round-trip requirements: re-ingest of an unmodified bundle MUST reproduce
the source state hash. A modified bundle ingests as a **proposal**
(§4.3), authored by the ingesting principal. A human edit to a file is a
governed mutation like any other.

The write path is deliberately narrow. The editable surface is:
front-matter fields that project attributes, the designated long-text
attributes in the prose body, and typed wiki-link edges. Each modified
file maps to `update` operations on exactly the objects it projects. An
edit outside this surface, or one that cannot map to schema-valid
operations, fails ingest with a report that names the file and the
reason, and the rest of the bundle still ingests. Implementations MAY
support richer mappings and MUST declare them as part of the binding.

## 7.3 Parquet binding (optional)

This is the analytical projection, for columnar and tabular engines:

| Table | Contents |
|---|---|
| `nodes` | id, rev, type, flattened common attributes, JSON overflow column |
| `edges` | id, rev, type, from, to, attributes |
| `classifications` | subject, term, version, asserted_by, basis |
| `documents` | id, content_hash, media_type, locator |
| `changesets` | hash, parents (list), author, timestamp, op-count, schema_context |
| `lineage` | object rev, derived_from refs, method, tool |

Partitioning by type and by changeset time is RECOMMENDED. The binding is
append-friendly: new changesets append rows, and state tables are
rebuildable from the log tables.

Declared losses: signatures verify against wire-form bytes, so Parquet
alone cannot re-verify. The export ships with the state-hash manifest,
and it supports analysis while verification still requires the wire-form
log.

## 7.4 Round-trip conformance

An L0 implementation that claims a binding MUST pass these tests:

1. **Faithful bindings** (wire form). Log to projection to log. The
   changesets are byte-identical and the state hash is identical.
2. **Declared-lossy bindings** (markdown, Parquet). State to projection
   to state′. The hash of state′ equals the source state hash over the
   binding's declared-preserved object set. Every loss must be one the
   binding declared. An undeclared loss is non-conformance.
