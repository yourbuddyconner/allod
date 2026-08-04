/**
 * Rust ↔ TypeScript interop tests.
 *
 * Direction 1 (TS → Rust): build a graph via @allod/core + fsBackend, then
 * spawn the Rust CLI `verify` command against the written directory and
 * assert exit 0 and the VERIFIED success marker.
 *
 * Direction 2 (Rust → TS): drive the CLI (init, agent-add, note, verify) to
 * build an admitted graph in a temp directory, then open it from TypeScript
 * via AllodGraph + fsBackend and assert g.verify().ok === true and that the
 * TS state hash starts with the short hash printed by the CLI.
 *
 * ALLOD_BIN: path to a prebuilt allod binary. Falls back to
 * `cargo run -p allod --release --` (slow, but works in dev).
 */

import { describe, expect, test } from "vitest";
import { mkdtempSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { AllodGraph } from "../pkg/allod_wasm.js";
import { fsBackend } from "../js/store.js";

// Workspace root is three directories above this file (crates/allod-wasm/tests/).
const WORKSPACE_ROOT = resolve(
  fileURLToPath(import.meta.url),
  "..",
  "..",
  "..",
  ".."
);

// ---------------------------------------------------------------------------
// CLI helper
// ---------------------------------------------------------------------------

/**
 * Resolve the allod binary.
 *
 * In CI the binary is prebuilt and its path is passed via ALLOD_BIN.
 * In local dev it falls back to `cargo run -p allod --release --` so the
 * test can be run with `pnpm test` without a separate build step.
 */
function alodBin(): { cmd: string; baseArgs: string[] } {
  const bin = process.env.ALLOD_BIN;
  if (bin) {
    return { cmd: bin, baseArgs: [] };
  }
  // Locate workspace root relative to this file's package directory.
  // __dirname is crates/allod-wasm/tests when run via tsx/vitest.
  return {
    cmd: "cargo",
    baseArgs: ["run", "-p", "allod", "--release", "--"],
  };
}

/** Run a CLI sub-command synchronously; throw on non-zero exit. */
function cli(...args: string[]): string {
  const { cmd, baseArgs } = alodBin();
  const result = spawnSync(cmd, [...baseArgs, ...args], {
    encoding: "utf8",
    // Run from workspace root so the CLI finds ontologies/ by default.
    cwd: WORKSPACE_ROOT,
    // Allow up to 120 s in case cargo needs to (re)build.
    timeout: 120_000,
  });
  if (result.error) throw result.error;
  const output = (result.stdout ?? "") + (result.stderr ?? "");
  if (result.status !== 0) {
    throw new Error(
      `allod ${args.join(" ")} exited ${result.status}:\n${output}`
    );
  }
  return output;
}

/** Run `verify` and return { exitCode, stdout }. Does NOT throw on failure. */
function cliVerifyRaw(dir: string): { exitCode: number; stdout: string } {
  const { cmd, baseArgs } = alodBin();
  const result = spawnSync(cmd, [...baseArgs, "verify", dir], {
    encoding: "utf8",
    cwd: WORKSPACE_ROOT,
    timeout: 120_000,
  });
  return {
    exitCode: result.status ?? 1,
    stdout: (result.stdout ?? "") + (result.stderr ?? ""),
  };
}

// ---------------------------------------------------------------------------
// Direction 1: TS → Rust
// ---------------------------------------------------------------------------

describe("TS → Rust interop", () => {
  test("graph built by AllodGraph passes CLI verify", async () => {
    const dir = mkdtempSync(join(tmpdir(), "allod-ts-to-rust-"));
    const backend = fsBackend(dir);
    const g = new AllodGraph([], backend.persist);

    // Founding loop — mirrors memory-flow.test.ts
    await g.init("conner", "memory");
    await g.principal_add("jarvis", "agent", "conner");
    const note = await g.note("jarvis", "prefers tea");
    expect(note.admission.Admitted).toBeDefined();

    const pref = await g.propose_preference(
      "jarvis",
      "tea over coffee",
      "hard",
      note.note_id
    );
    expect(pref.admission.Held).toBeDefined();

    const decided = await g.decide(pref.hash, "conner", "approve");
    expect(decided.Admitted).toBeDefined();

    const tsReport = g.verify();
    expect(tsReport.ok).toBe(true);

    // Now let the Rust CLI re-verify the same directory written by fsBackend.
    const { exitCode, stdout } = cliVerifyRaw(dir);

    expect(exitCode).toBe(0);
    // The CLI prints "VERIFIED: N changesets, levels 1-3 (§5.3)" on success.
    expect(stdout).toMatch(/VERIFIED:/);
  });
});

// ---------------------------------------------------------------------------
// Direction 2: Rust → TS
// ---------------------------------------------------------------------------

describe("Rust → TS interop", () => {
  test("graph built by CLI passes AllodGraph verify with governance cycle, hashes agree", () => {
    const dir = mkdtempSync(join(tmpdir(), "allod-rust-to-ts-"));

    // Build the graph via the CLI, mirroring Direction 1's flow:
    // init → agent-add → note → propose-preference → approve.
    // This exercises the full governance cycle and ensures every changeset
    // (including governed writes) is properly admitted.
    cli("init", dir, "--owner", "conner");
    cli("agent-add", dir, "jarvis", "--by", "conner");
    cli("note", dir, "--as", "jarvis", "prefers tea");

    // Propose and approve a preference to test the governance cycle.
    cli("propose-preference", dir, "--as", "jarvis", "--statement", "prefers tea", "--strength", "soft");
    const proposalsDir = join(dir, ".allod/proposals");
    const fullHash = readdirSync(proposalsDir)
      .filter((f) => !f.endsWith(".evidence.yaml"))
      .map((f) => "sha256:" + f.replace(".yaml", ""))[0];
    cli("approve", dir, fullHash, "--as", "conner");

    // Capture the short state hash from the CLI.
    const { exitCode: verifyExit, stdout: verifyOut } = cliVerifyRaw(dir);
    expect(verifyExit).toBe(0);
    expect(verifyOut).toMatch(/VERIFIED:/);

    // "  state hash <12-char-hex>" — extract the 12-char prefix.
    const hashMatch = verifyOut.match(/state hash ([0-9a-f]{12})/);
    expect(hashMatch).not.toBeNull();
    const cliShortHash = hashMatch![1];

    // Open the same directory from TypeScript.
    const backend = fsBackend(dir);
    const g = new AllodGraph(backend.load(), backend.persist);

    const tsReport = g.verify();
    expect(tsReport.ok).toBe(true);

    // The TS state_hash is "sha256:<64-char-hex>"; the CLI prints the first
    // 12 hex characters.  Strip the "sha256:" prefix and assert the hex starts
    // with the CLI's short hash.
    const tsStateHash = g.state().state_hash;
    const tsHex = tsStateHash.startsWith("sha256:")
      ? tsStateHash.slice("sha256:".length)
      : tsStateHash;
    expect(tsHex.startsWith(cliShortHash)).toBe(true);
  });
});
