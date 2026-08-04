import { describe, expect, test } from "vitest";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { AllodGraph } from "../pkg/allod_wasm.js";
import { fsBackend } from "../js/store.js";

// Note on representation: serde-wasm-bindgen serialises Rust enum variants
// using serde's "external tagging" convention:
//   Admission::Admitted { hash, matched_rules } → { Admitted: { hash, matched_rules } }
//   Admission::Held    { hash, checklist }       → { Held:    { hash, checklist } }
//   DecisionOutcome::Admitted { degraded }       → { Admitted: { degraded } }
//   DecisionOutcome::Rejected                    → "Rejected"
//
// The test assertions below follow this representation.

/** Generate a UUID v4 string (mirrors the Rust ops::uuid4 helper). */
function uuid4(): string {
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20, 32)}`;
}

test("the founding loop, from TypeScript", async () => {
  const dir = mkdtempSync(join(tmpdir(), "allod-wasm-"));
  const backend = fsBackend(dir);
  const g = new AllodGraph([], backend.persist);

  await g.init("conner", "memory");
  await g.principal_add("jarvis", "agent", "conner");

  const note = await g.note("jarvis", "prefers tea");
  // Scratch notes are admitted immediately under scratch-is-free
  expect(note.admission.Admitted).toBeDefined();

  // NOTE: the brief used "strong" but the memory/Preference ontology enforces enum<hard|soft>.
  // "hard" is the correct value.
  const pref = await g.propose_preference("jarvis", "tea over coffee", "hard", note.note_id);
  // Preference proposals are held for owner decision
  expect(pref.admission.Held).toBeDefined();

  const decided = await g.decide(pref.hash, "conner", "approve");
  // DecisionOutcome::Admitted { degraded: [] } → { Admitted: { degraded: [] } }
  expect(decided.Admitted).toBeDefined();

  const report = g.verify();
  expect(report.ok).toBe(true);

  // Resume from persisted state: a second instance reads the same dir
  const g2 = new AllodGraph(backend.load(), backend.persist);
  expect(g2.state().state_hash).toEqual(g.state().state_hash);
});

test("describe_schema lists memory/Note type", async () => {
  const dir = mkdtempSync(join(tmpdir(), "allod-wasm-schema-"));
  const backend = fsBackend(dir);
  const g = new AllodGraph([], backend.persist);
  await g.init("alice", "memory");

  // describe_schema returns a SchemaDescription: { entity_types, edge_types, terms }
  const schema = g.describe_schema();
  expect(schema).toBeDefined();
  // The memory ontology defines Note, Preference, Routine, Commitment, Interest
  expect(Array.isArray(schema.entity_types)).toBe(true);
  const names: string[] = schema.entity_types.map((t: { name: string }) => t.name);
  expect(names.some((n) => n.includes("Note"))).toBe(true);
  // Schema should also include terms and edge_types arrays
  expect(Array.isArray(schema.terms)).toBe(true);
  expect(Array.isArray(schema.edge_types)).toBe(true);
});

test("install_package with tiny ontology is Held or Admitted", async () => {
  const dir = mkdtempSync(join(tmpdir(), "allod-wasm-install-"));
  const backend = fsBackend(dir);
  const g = new AllodGraph([], backend.persist);
  await g.init("alice", "memory");

  // docs_yaml is a YAML mapping where each value is the ontology doc object.
  // The memory policy requires owner review for define-type ops (schema-changes-are-serious),
  // so the result will be Held.
  const docsYaml = `test:
  ontology: test
  entity_types:
    Widget:
      attributes:
        name:
          type: string
          required: true
`;

  const result = await g.install_package(docsYaml, "alice");
  // Under the memory policy, schema changes (define-type) require owner review.
  // Even when the owner is the author, a formal decision record is required,
  // so the changeset is Held.
  expect(result.Held !== undefined || result.Admitted !== undefined).toBe(true);
});

test("generic commit: memory/Note@1 admitted, memory/Preference@1 held", async () => {
  const dir = mkdtempSync(join(tmpdir(), "allod-wasm-commit-"));
  const backend = fsBackend(dir);
  const g = new AllodGraph([], backend.persist);

  await g.init("conner", "memory");
  await g.principal_add("jarvis", "agent", "conner");

  // --- commit 1: create a memory/Note@1 + scratch classification (mirrors flows::note) ---
  const noteId = uuid4();
  const noteOp = {
    create: {
      kind: "node",
      id: noteId,
      type: "memory/Note@1",
      attributes: { content: "committed note" },
    },
  };
  const noteClsOp = {
    create: {
      kind: "classification",
      id: uuid4(),
      subject: `node:${noteId}`,
      term: "workspace/scratch@1",
      asserted_by: "principal:jarvis",
      basis: "model-assisted",
    },
  };
  const noteResult = await g.commit("jarvis", "Scratch note", [noteOp, noteClsOp], []);
  // scratch notes are admitted immediately under scratch-is-free
  expect(noteResult.Admitted).toBeDefined();

  // --- commit 2: create a memory/Preference@1 classified work@1 (mirrors flows::propose_preference) ---
  const prefId = uuid4();
  const prefOp = {
    create: {
      kind: "node",
      id: prefId,
      type: "memory/Preference@1",
      attributes: { statement: "generic commit works", strength: "hard" },
    },
  };
  const prefClsOp = {
    create: {
      kind: "classification",
      id: uuid4(),
      subject: `node:${prefId}`,
      term: "work@1",
      asserted_by: "principal:jarvis",
      basis: "model-assisted",
    },
  };
  const prefResult = await g.commit("jarvis", "Preference proposal", [prefOp, prefClsOp], []);
  // Preference proposals are held for owner review
  expect(prefResult.Held).toBeDefined();
});

test("proposal_get returns the full changeset for a held proposal", async () => {
  const dir = mkdtempSync(join(tmpdir(), "allod-wasm-proposal-get-"));
  const backend = fsBackend(dir);
  const g = new AllodGraph([], backend.persist);

  await g.init("conner", "memory");
  await g.principal_add("jarvis", "agent", "conner");

  const pref = await g.propose_preference("jarvis", "tea over coffee", "hard", undefined);
  expect(pref.admission.Held).toBeDefined();
  const hash = pref.hash as string;

  const cs = g.proposal_get(hash) as Record<string, unknown>;
  expect(cs).toBeDefined();
  expect(cs).not.toBeNull();
  // The changeset must have a hash and an author (principal field) from storage.
  const csKeys = Object.keys(cs);
  expect(csKeys.length).toBeGreaterThan(0);
  expect(cs.hash).toBeDefined();
  expect(cs.author).toBeDefined();
});

test("object_get returns content+rev+deleted for a live node, null for unknown", async () => {
  const dir = mkdtempSync(join(tmpdir(), "allod-wasm-object-get-"));
  const backend = fsBackend(dir);
  const g = new AllodGraph([], backend.persist);

  await g.init("conner", "memory");
  await g.principal_add("jarvis", "agent", "conner");

  const note = await g.note("jarvis", "hello world");
  expect(note.admission.Admitted).toBeDefined();
  const noteId = note.note_id as string;

  // Live node must be returned
  const obj = g.object_get("node", noteId) as { content: unknown; rev: string; deleted: boolean } | null;
  expect(obj).not.toBeNull();
  expect(typeof obj!.rev).toBe("string");
  expect(obj!.rev.length).toBeGreaterThan(0);
  expect(obj!.deleted).toBe(false);
  expect(obj!.content).toBeDefined();

  // Unknown id must return null
  const missing = g.object_get("node", "00000000-0000-0000-0000-000000000000");
  expect(missing).toBeNull();
});
