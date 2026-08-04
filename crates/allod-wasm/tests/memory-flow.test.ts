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
