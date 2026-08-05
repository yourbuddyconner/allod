import { describe, expect, test } from "vitest";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createPrivateKey, sign as nodeSign } from "node:crypto";
import { AllodGraph } from "../pkg/allod_wasm.js";
import { fsBackend } from "../js/store.js";

/**
 * Sign `payload` (a sha256:<hex> string) with an ed25519 secret key.
 * `secretHex` is a 64-hex-char string (the 32-byte raw ed25519 seed in hex),
 * as stored in keys/<name>.yaml under the "secret" field.
 *
 * Returns "sig:ed25519:<hex>".
 */
function ed25519Sign(secretHex: string, payload: string): string {
  // PKCS#8 DER header for a bare ed25519 private key (RFC 8410).
  const header = Buffer.from("302e020100300506032b657004220420", "hex");
  const secretBytes = Buffer.from(secretHex, "hex");
  const der = Buffer.concat([header, secretBytes]);
  const key = createPrivateKey({ key: der, format: "der", type: "pkcs8" });
  const sig = nodeSign(null, Buffer.from(payload, "utf8"), key);
  return `sig:ed25519:${sig.toString("hex")}`;
}

/**
 * Extract the owner's secret key from a dump (Array<[path, content]>).
 * Looks for "keys/<name>.yaml" and parses the "secret:" field.
 */
function extractSecret(dump: [string, string][], name: string): string {
  const entry = dump.find(([path]) => path === `keys/${name}.yaml`);
  if (!entry) throw new Error(`keys/${name}.yaml not found in dump`);
  const match = entry[1].match(/^secret:\s*([0-9a-f]+)/m);
  if (!match) throw new Error(`secret field not found in keys/${name}.yaml`);
  return match[1];
}

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

test("entity_context returns classifications and edges for a node", async () => {
  const dir = mkdtempSync(join(tmpdir(), "allod-wasm-entity-ctx-"));
  const backend = fsBackend(dir);
  const g = new AllodGraph([], backend.persist);

  await g.init("conner", "memory");
  await g.principal_add("jarvis", "agent", "conner");

  // Create two nodes (scratch notes are admitted immediately)
  const noteAId = uuid4();
  const noteBId = uuid4();
  const clsIdA = uuid4();
  const edgeId = uuid4();
  const clsIdEdge = uuid4();

  // Create noteA with scratch classification
  const r1 = await g.commit("jarvis", "Scratch note A", [
    { create: { kind: "node", id: noteAId, type: "memory/Note@1", attributes: { content: "note A" } } },
    { create: { kind: "classification", id: clsIdA, subject: `node:${noteAId}`, term: "workspace/scratch@1", asserted_by: "principal:jarvis", basis: "model-assisted" } },
  ], []);
  expect(r1.Admitted).toBeDefined();

  // Create noteB with scratch classification
  const clsIdB = uuid4();
  const r2 = await g.commit("jarvis", "Scratch note B", [
    { create: { kind: "node", id: noteBId, type: "memory/Note@1", attributes: { content: "note B" } } },
    { create: { kind: "classification", id: clsIdB, subject: `node:${noteBId}`, term: "workspace/scratch@1", asserted_by: "principal:jarvis", basis: "model-assisted" } },
  ], []);
  expect(r2.Admitted).toBeDefined();

  // Create an edge from noteA to noteB with scratch classification on the edge changeset
  const r3 = await g.commit("jarvis", "Relate memory/relates_to@1", [
    { create: { kind: "edge", id: edgeId, type: "memory/relates_to@1", from: `node:${noteAId}`, to: `node:${noteBId}`, attributes: {} } },
    { create: { kind: "classification", id: clsIdEdge, subject: `node:${noteAId}`, term: "workspace/scratch@1", asserted_by: "principal:jarvis", basis: "model-assisted" } },
  ], []);
  // relate changeset may be Admitted or Held depending on policy; just check it doesn't error
  expect(r3.Admitted !== undefined || r3.Held !== undefined).toBe(true);

  // entity_context for noteA
  const ctx = g.entity_context(noteAId) as {
    classifications: Array<{ term: string; asserted_by: string; basis: string }>;
    edges_out: Array<{ id: string; type: string; to: string; attributes: Record<string, unknown> }>;
    edges_in: Array<{ id: string; type: string; from: string; attributes: Record<string, unknown> }>;
  } | null;

  expect(ctx).not.toBeNull();
  // classifications: workspace/scratch@1 was asserted on noteA
  expect(Array.isArray(ctx!.classifications)).toBe(true);
  expect(ctx!.classifications.length).toBeGreaterThan(0);
  const cls = ctx!.classifications.find((c) => c.term === "workspace/scratch@1");
  expect(cls).toBeDefined();
  expect(cls!.asserted_by).toBe("principal:jarvis");
  expect(cls!.basis).toBe("model-assisted");

  // edges_out: the edge from noteA to noteB (only if r3 was admitted)
  if (r3.Admitted) {
    expect(Array.isArray(ctx!.edges_out)).toBe(true);
    expect(ctx!.edges_out.length).toBeGreaterThan(0);
    const edge = ctx!.edges_out.find((e) => e.id === edgeId);
    expect(edge).toBeDefined();
    expect(edge!.type).toBe("memory/relates_to@1");
    expect(edge!.to).toBe(`node:${noteBId}`);

    // entity_context for noteB should have edges_in
    const ctxB = g.entity_context(noteBId) as typeof ctx;
    expect(ctxB).not.toBeNull();
    expect(Array.isArray(ctxB!.edges_in)).toBe(true);
    expect(ctxB!.edges_in.length).toBeGreaterThan(0);
    const edgeIn = ctxB!.edges_in.find((e) => e.id === edgeId);
    expect(edgeIn).toBeDefined();
    expect(edgeIn!.from).toBe(`node:${noteAId}`);
  }

  // unknown node returns null
  expect(g.entity_context("00000000-0000-0000-0000-000000000000")).toBeNull();
});

test("commit with sign_envelope=true → Held; decide approve → Admitted", async () => {
  const dir = mkdtempSync(join(tmpdir(), "allod-wasm-envelope-"));
  const backend = fsBackend(dir);
  const g = new AllodGraph([], backend.persist);

  await g.init("owner", "memory");
  await g.principal_add("agent", "agent", "owner");

  const prefId = uuid4();

  // commit with sign_envelope=true builds and attaches an attestation envelope
  const admission = await g.commit(
    "agent",
    "Create memory/Preference@1",
    [
      {
        create: {
          kind: "node",
          id: prefId,
          type: "memory/Preference@1",
          attributes: { statement: "prefers dark mode", strength: "soft" },
          provenance: {
            derived_by: "principal:agent",
            method: "model-assisted",
            tool: "freehold@0.1",
          },
        },
      },
    ],
    [],
    true  // sign_envelope
  );

  // Preference without scratch classification → Held
  expect(admission.Held).toBeDefined();
  const hash: string = (admission as { Held: { hash: string } }).Held.hash;

  // Owner approves — signed envelope satisfies model-assisted-needs-signed-envelope → Admitted
  const outcome = await g.decide(hash, "owner", "approve");
  expect((outcome as { Admitted?: { degraded: string[] } }).Admitted).toBeDefined();
  expect(Array.isArray((outcome as { Admitted: { degraded: string[] } }).Admitted.degraded)).toBe(true);
});

test("persist callback rejection propagates to the mutating call (non-vacuous)", async () => {
  // The wasm contract: do_persist() awaits the JS persist callback. If the callback
  // rejects, the mutating method (e.g. init) must also reject — not silently succeed.
  // This guards against persist failures being swallowed.

  const failingPersist = async (_dump: [string, string][]) => {
    throw new Error("simulated persist failure");
  };

  const g = new AllodGraph([], failingPersist);

  // init() calls do_persist() after writing the genesis changeset.
  // The rejection must propagate out of init().
  await expect(g.init("owner", "memory")).rejects.toThrow("simulated persist failure");

  // note() also calls do_persist() — test that too.
  // We need a graph that was initialised without a failing persist callback first.
  const dir = mkdtempSync(join(tmpdir(), "allod-wasm-persist-err-"));
  const backend = fsBackend(dir);
  const gOk = new AllodGraph([], backend.persist.bind(backend));
  await gOk.init("owner", "memory");
  await gOk.principal_add("agent", "agent", "owner");

  // Now swap in the failing persist: re-open the graph with a failing callback
  const gFail = new AllodGraph(backend.load(), failingPersist);
  await expect(gFail.note("agent", "this note must not persist")).rejects.toThrow(
    "simulated persist failure"
  );
});

// ---------------------------------------------------------------------------
// Task 6: two-phase signing seam
// ---------------------------------------------------------------------------

describe("two-phase commit (commit_payload + commit_signed)", () => {
  test("commit_payload returns changeset without signature + hash; commit_signed with external sig admits scratch note", async () => {
    let latestDump: [string, string][] = [];
    const capturingPersist = async (dump: [string, string][]) => {
      latestDump = dump;
    };

    const g = new AllodGraph([], capturingPersist);
    await g.init("owner", "memory");
    await g.principal_add("agent", "agent", "owner");

    // Capture dump after principal_add so we have agent's key
    const agentSecret = extractSecret(latestDump, "agent");

    // Use a scratch note (workspace/scratch@1 classification) — scratch-is-free admits immediately.
    const noteId = uuid4();
    const ops = [
      {
        create: {
          kind: "node",
          id: noteId,
          type: "memory/Note@1",
          attributes: { content: "two-phase commit works" },
        },
      },
      {
        create: {
          kind: "classification",
          id: uuid4(),
          subject: `node:${noteId}`,
          term: "workspace/scratch@1",
          asserted_by: "principal:agent",
          basis: "model-assisted",
        },
      },
    ];

    // Phase 1: build the changeset payload without signing (read-only)
    const phase1 = g.commit_payload("agent", "Two-phase scratch note", ops);
    expect(phase1).toBeDefined();
    expect(typeof phase1.hash).toBe("string");
    expect(phase1.hash).toMatch(/^sha256:/);
    expect(phase1.changeset).toBeDefined();
    // The changeset must NOT have a signature field yet
    expect((phase1.changeset as Record<string, unknown>).signature).toBeUndefined();

    // Phase 2: sign the hash externally and submit
    const signature = ed25519Sign(agentSecret, phase1.hash as string);
    const outcome = await g.commit_signed(phase1.changeset, signature, []);
    // Scratch note is admitted immediately under scratch-is-free
    expect(outcome.Admitted).toBeDefined();

    // verify() must pass after admission
    const report = g.verify();
    expect(report.ok).toBe(true);
  });

  test("commit_signed with envelope satisfies model-assisted-needs-signed-envelope", async () => {
    let latestDump: [string, string][] = [];
    const capturingPersist = async (dump: [string, string][]) => {
      latestDump = dump;
    };

    const g = new AllodGraph([], capturingPersist);
    await g.init("owner", "memory");
    await g.principal_add("agent", "agent", "owner");
    const agentSecret = extractSecret(latestDump, "agent");

    const prefId = uuid4();
    const ops = [
      {
        create: {
          kind: "node",
          id: prefId,
          type: "memory/Preference@1",
          attributes: { statement: "envelope two-phase", strength: "hard" },
          provenance: { derived_by: "principal:agent", method: "model-assisted", tool: "test@0.1" },
        },
      },
    ];

    // Phase 1: commit payload
    const phase1 = g.commit_payload("agent", "Envelope two-phase test", ops);
    const csHash = phase1.hash as string;
    const csSignature = ed25519Sign(agentSecret, csHash);

    // Get the envelope payload to sign
    const envPhase = g.envelope_payload("agent", csHash);
    expect(envPhase).toBeDefined();
    expect(typeof envPhase.payload).toBe("string");
    const envSignature = ed25519Sign(agentSecret, envPhase.payload as string);

    // Attach signature to envelope
    const envelope = { ...(envPhase.envelope as object), signature: envSignature };

    // Phase 2: commit_signed with envelope — Preference without scratch → Held for owner review.
    // The signed envelope satisfies model-assisted-needs-signed-envelope but owner decide still needed.
    const outcome = await g.commit_signed(phase1.changeset, csSignature, [envelope]);
    expect(outcome.Held).toBeDefined();
  });
});

describe("two-phase decide (decide_payload + decide_with_record)", () => {
  test("decide_payload returns record+payload; decide_with_record with signed record admits proposal", async () => {
    let latestDump: [string, string][] = [];
    const capturingPersist = async (dump: [string, string][]) => {
      latestDump = dump;
    };

    const g = new AllodGraph([], capturingPersist);
    await g.init("owner", "memory");
    await g.principal_add("agent", "agent", "owner");
    const ownerSecret = extractSecret(latestDump, "owner");

    // Create a held proposal
    const pref = await g.propose_preference("agent", "decide two-phase", "hard", undefined);
    expect(pref.admission.Held).toBeDefined();
    const hash = pref.hash as string;

    // Phase 1: read-only — get unsigned decision record + payload
    const phase1 = g.decide_payload(hash, "approve");
    expect(phase1).toBeDefined();
    expect(typeof phase1.payload).toBe("string");
    expect(phase1.record).toBeDefined();
    expect((phase1.record as Record<string, unknown>).kind).toBe("decision-record");
    expect((phase1.record as Record<string, unknown>).verdict).toBe("approve");

    // Sign externally
    const ownerSig = ed25519Sign(ownerSecret, phase1.payload as string);

    // Attach decider to record
    const record = phase1.record as Record<string, unknown>;
    const signedRecord = {
      ...record,
      deciders: [{ principal: "principal:owner", signature: ownerSig }],
    };

    // Phase 2: mutating — submit the signed record
    const outcome = await g.decide_with_record(hash, signedRecord);
    expect(outcome.Admitted).toBeDefined();
  });
});

describe("envelope_payload (read-only)", () => {
  test("returns envelope without signature and the payload string to sign", async () => {
    const g = new AllodGraph([], async () => {});
    await g.init("owner", "memory");

    const fakeHash = "sha256:aabbcc001122334455667788990011223344556677889900112233445566778899";
    const result = g.envelope_payload("owner", fakeHash);
    expect(result).toBeDefined();
    expect(typeof result.payload).toBe("string");
    expect(result.payload.length).toBeGreaterThan(0);
    expect(result.envelope).toBeDefined();
    // Envelope must NOT have a signature field
    expect((result.envelope as Record<string, unknown>).signature).toBeUndefined();
    // Envelope must reference the hash
    const env = result.envelope as Record<string, unknown>;
    expect((env.statement as Record<string, unknown>).changeset_hash).toBe(fakeHash);
  });
});

/** Install a git-substrate policy and approve it so it takes effect. */
async function installAndApproveGitPolicy(
  g: AllodGraph,
  ownerSecret: string
): Promise<void> {
  // roles: binds principal:owner into the "owner" role so that
  // git_satisfaction can verify that the owner's signature satisfies the
  // reviewer requirement.
  const gitPolicy = `roles:
  owner:
    - principal:owner
rules:
  - name: protected-src
    select:
      substrate: git
      path: "src/**"
    require:
      reviewers:
        - role: owner
          quorum: 1
`;
  const policyResult = await g.install_policy(gitPolicy, "owner");
  // Under memory policy, set-policy requires owner decision.
  if ((policyResult as Record<string, unknown>).Held) {
    const policyHash = (
      (policyResult as { Held: { hash: string } }).Held
    ).hash;
    // Use two-phase decide to approve the policy install
    const phase1 = g.decide_payload(policyHash, "approve");
    const ownerSig = ed25519Sign(ownerSecret, phase1.payload as string);
    const record = {
      ...(phase1.record as object),
      deciders: [{ principal: "principal:owner", signature: ownerSig }],
    };
    await g.decide_with_record(policyHash, record);
  }
}

describe("git evaluation bindings", () => {
  test("git_checklist matches path-glob rules in policy", async () => {
    let latestDump: [string, string][] = [];
    const capturingPersist = async (dump: [string, string][]) => {
      latestDump = dump;
    };
    const g = new AllodGraph([], capturingPersist);
    await g.init("owner", "memory");
    const ownerSecret = extractSecret(latestDump, "owner");

    await installAndApproveGitPolicy(g, ownerSecret);

    const ops = [["update", "src/main.rs"], ["update", "README.md"]];
    const result = g.git_checklist("my-repo", "refs/heads/main", ops);
    expect(result).toBeDefined();
    // "protected-src" matches src/main.rs
    expect(Array.isArray(result.matched)).toBe(true);
    expect((result.matched as string[]).includes("protected-src")).toBe(true);
    // checklist serialised
    expect(result.checklist).toBeDefined();
  });

  test("git_satisfaction returns unmet reviewers when no decisions provided", async () => {
    let latestDump: [string, string][] = [];
    const capturingPersist = async (dump: [string, string][]) => {
      latestDump = dump;
    };
    const g = new AllodGraph([], capturingPersist);
    await g.init("owner", "memory");
    const ownerSecret = extractSecret(latestDump, "owner");

    await installAndApproveGitPolicy(g, ownerSecret);

    const ops = [["update", "src/main.rs"]];
    const checklistResult = g.git_checklist("my-repo", "refs/heads/main", ops);
    // No decisions → reviewer requirement unmet
    const unmetResult = g.git_satisfaction(
      "git:abc123def456",
      checklistResult.checklist,
      []
    );
    expect(unmetResult).toBeDefined();
    expect(Array.isArray(unmetResult.unmet)).toBe(true);
    expect((unmetResult.unmet as string[]).length).toBeGreaterThan(0);
  });

  test("git_decision_payload returns record+payload for signing", async () => {
    const g = new AllodGraph([], async () => {});
    await g.init("owner", "memory");

    const subject = "git:deadbeef1234567890";
    const result = g.git_decision_payload(subject, "approve");
    expect(result).toBeDefined();
    expect(typeof result.payload).toBe("string");
    expect(result.payload.length).toBeGreaterThan(0);
    const record = result.record as Record<string, unknown>;
    expect(record.kind).toBe("decision-record");
    expect(record.subject).toBe(subject);
    expect(record.verdict).toBe("approve");
    expect(record.deciders).toBeUndefined(); // not yet signed
  });

  test("git_decision_attach appends decider to record", async () => {
    const g = new AllodGraph([], async () => {});
    await g.init("owner", "memory");

    const subject = "git:cafebabe0000";
    const phase1 = g.git_decision_payload(subject, "approve");
    const fakeSignature = "sig:ed25519:" + "aa".repeat(64);
    const withDecider = g.git_decision_attach(
      phase1.record,
      "owner",
      fakeSignature
    );
    expect(withDecider).toBeDefined();
    const rec = withDecider as Record<string, unknown>;
    expect(Array.isArray(rec.deciders)).toBe(true);
    const deciders = rec.deciders as Array<Record<string, unknown>>;
    expect(deciders.length).toBe(1);
    expect(deciders[0].principal).toBe("principal:owner");
    expect(deciders[0].signature).toBe(fakeSignature);
  });

  test("git_satisfaction satisfied branch: owner signs decision → unmet becomes empty", async () => {
    // The git policy in installAndApproveGitPolicy requires `role: owner` for src/**.
    // The "owner" principal is the graph owner, which holds the owner role.
    // Steps:
    //   1. Install & approve git policy with owner reviewer requirement.
    //   2. git_checklist for a src/** path → has reviewer requirement.
    //   3. git_satisfaction with no decisions → unmet non-empty.
    //   4. git_decision_payload → sign with owner key → git_decision_attach.
    //   5. git_satisfaction with the signed decision → unmet empty.
    let latestDump: [string, string][] = [];
    const capturingPersist = async (dump: [string, string][]) => {
      latestDump = dump;
    };
    const g = new AllodGraph([], capturingPersist);
    await g.init("owner", "memory");
    const ownerSecret = extractSecret(latestDump, "owner");

    await installAndApproveGitPolicy(g, ownerSecret);

    const subject = "git:abc123def456";
    const ops = [["update", "src/lib.rs"]];
    const checklistResult = g.git_checklist("my-repo", "refs/heads/main", ops);
    expect((checklistResult.matched as string[]).includes("protected-src")).toBe(true);

    // With no decisions: unmet must be non-empty.
    const unmetBefore = g.git_satisfaction(subject, checklistResult.checklist, []);
    expect((unmetBefore.unmet as string[]).length).toBeGreaterThan(0);

    // Build a signed decision record.
    const phase1 = g.git_decision_payload(subject, "approve");
    const ownerSig = ed25519Sign(ownerSecret, phase1.payload as string);
    const signedRecord = g.git_decision_attach(phase1.record, "owner", ownerSig);

    // With the signed owner decision: unmet must be empty.
    const unmetAfter = g.git_satisfaction(subject, checklistResult.checklist, [signedRecord]);
    expect((unmetAfter.unmet as string[]).length).toBe(0);
  });

  test("git_checklist matches region-keyed rules via code graph nodes", async () => {
    // Mirrors the Rust evaluate_git_region_reaches_through_derived_paths test in
    // crates/allod-core/src/policy.rs (§8.3 region reach).
    //
    // Graph structure:
    //   node:f1  — code/SourceFile@1, path "src/pay.rs"
    //   node:fn1 — code/Function@1
    //   edge:e1  — code/declares@1, from node:f1 → node:fn1
    //   classification — term "security/critical" on node:fn1
    //
    // Policy: permissive default + one git rule selecting region "security/critical".
    // Expected: ops touching src/pay.rs → rule matched; ops on src/other.rs → not matched.
    let latestDump: [string, string][] = [];
    const capturingPersist = async (dump: [string, string][]) => {
      latestDump = dump;
    };
    const g = new AllodGraph([], capturingPersist);
    await g.init("owner", "memory");
    const ownerSecret = extractSecret(latestDump, "owner");

    // Helper: approve a Held changeset as owner.
    const approveHeld = async (result: unknown) => {
      const r = result as Record<string, unknown>;
      if (r.Held) {
        const hash = (r.Held as { hash: string }).hash;
        const phase1 = g.decide_payload(hash, "approve");
        const sig = ed25519Sign(ownerSecret, phase1.payload as string);
        const record = {
          ...(phase1.record as object),
          deciders: [{ principal: "principal:owner", signature: sig }],
        };
        await g.decide_with_record(hash, record);
      }
    };

    // Step 1: Install minimal code-schema types and the security taxonomy.
    // Both will be Held under the memory policy (schema-changes-are-serious),
    // then approved by the owner.
    //
    // We install a stripped-down "code" ontology (no external-ref blob attribute,
    // no import chain) whose type names match those used by path_regions() in
    // allod-core/src/policy.rs:  code/SourceFile, code/Function, code/declares.
    const codeSchemaYaml = `code:
  ontology: code
  version: 1
  entity_types:
    SourceFile:
      attributes:
        path: { type: string, required: true }
    Function:
      attributes:
        name: { type: string, required: true }
  edge_types:
    declares:
      domain: SourceFile
      range: [Function]
      cardinality: one-to-many
security-taxonomy:
  taxonomy: security-taxonomy
  version: 1
  terms:
    - { name: security, parents: [] }
    - { name: "security/critical", parents: [security] }
`;
    const schemaResult = await g.install_package(codeSchemaYaml, "owner");
    await approveHeld(schemaResult);

    // Step 2: Install a permissive policy that has a git region rule.
    // default_posture: permissive means node/edge/classification commits by the
    // owner are admitted immediately without review after this policy takes effect.
    const regionPolicy = `policy: region-git-test
version: 1
default_posture: permissive
roles:
  owner:
    - principal:owner
rules:
  - name: region-critical
    select:
      substrate: git
      region: "security/critical"
    require:
      reviewers: { role: owner, quorum: 1 }
`;
    const policyResult = await g.install_policy(regionPolicy, "owner");
    // Under the memory policy, set-policy is schema-changes-are-serious → Held.
    await approveHeld(policyResult);

    // Now the permissive region policy is active.
    // Commit a code/SourceFile@1 node (path "src/pay.rs").
    const f1Id = uuid4();
    const f1Op = {
      create: {
        kind: "node",
        id: f1Id,
        type: "code/SourceFile@1",
        attributes: { path: "src/pay.rs" },
      },
    };
    const f1Result = await g.commit("owner", "Add SourceFile node", [f1Op], []);
    expect((f1Result as Record<string, unknown>).Admitted).toBeDefined();

    // Commit a code/Function@1 node.
    const fn1Id = uuid4();
    const fn1Op = {
      create: {
        kind: "node",
        id: fn1Id,
        type: "code/Function@1",
        attributes: { name: "pay" },
      },
    };
    const fn1Result = await g.commit("owner", "Add Function node", [fn1Op], []);
    expect((fn1Result as Record<string, unknown>).Admitted).toBeDefined();

    // Commit a code/declares@1 edge from f1 → fn1.
    const e1Id = uuid4();
    const e1Op = {
      create: {
        kind: "edge",
        id: e1Id,
        type: "code/declares@1",
        from: `node:${f1Id}`,
        to: `node:${fn1Id}`,
      },
    };
    const e1Result = await g.commit("owner", "Add declares edge", [e1Op], []);
    expect((e1Result as Record<string, unknown>).Admitted).toBeDefined();

    // Classify the Function node into "security/critical".
    // Under the permissive policy this is admitted immediately.
    const clsResult = await g.classify(fn1Id, "security/critical", "owner", "human-reviewed");
    expect((clsResult as Record<string, unknown>).Admitted).toBeDefined();

    // git_checklist: touching src/pay.rs must match region-critical
    // (the SourceFile is classified transitively via the declares edge).
    const hitsResult = g.git_checklist("my-repo", "refs/heads/main", [
      ["update", "src/pay.rs"],
    ]);
    expect(Array.isArray(hitsResult.matched)).toBe(true);
    expect((hitsResult.matched as string[]).includes("region-critical")).toBe(true);

    // git_checklist: touching an unclassified file must NOT match region-critical.
    const missResult = g.git_checklist("my-repo", "refs/heads/main", [
      ["update", "src/other.rs"],
    ]);
    expect(Array.isArray(missResult.matched)).toBe(true);
    expect((missResult.matched as string[]).includes("region-critical")).toBe(false);
  });
});
