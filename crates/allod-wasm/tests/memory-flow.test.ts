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
