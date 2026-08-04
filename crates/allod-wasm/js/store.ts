/**
 * Node.js filesystem persistence backend for AllodGraph.
 *
 * `fsBackend(graphDir)` returns:
 *   - `load()`: read every file under `graphDir/.allod/` into `Array<[relpath, text]>`
 *   - `persist(dump)`: write each `[relpath, text]` pair to `graphDir/.allod/<relpath>`,
 *     creating parent directories, then prune files not present in the dump.
 *
 * The on-disk layout produced here is the exact layout the Rust CLI reads via FsStore.
 */

import { readFileSync, readdirSync, statSync, mkdirSync, writeFileSync, unlinkSync, existsSync } from "node:fs";
import { readFile, writeFile, mkdir, unlink, readdir, stat } from "node:fs/promises";
import { join, dirname, relative } from "node:path";

export interface FsBackend {
  load(): Array<[string, string]>;
  persist(dump: Array<[string, string]>): Promise<void>;
}

/** Recursively collect all file paths under `dir`, returning them as an array. */
function collectFiles(dir: string): string[] {
  const out: string[] = [];
  if (!existsSync(dir)) return out;
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) {
      out.push(...collectFiles(full));
    } else {
      out.push(full);
    }
  }
  return out;
}

export function fsBackend(graphDir: string): FsBackend {
  const allodDir = join(graphDir, ".allod");

  return {
    /** Synchronous load — called in the AllodGraph constructor. */
    load(): Array<[string, string]> {
      const files = collectFiles(allodDir);
      const out: Array<[string, string]> = [];
      for (const abs of files) {
        const rel = relative(allodDir, abs);
        const text = readFileSync(abs, "utf8");
        out.push([rel, text]);
      }
      return out;
    },

    /** Async persist — called and awaited after every mutating AllodGraph call. */
    async persist(dump: Array<[string, string]>): Promise<void> {
      // Write (or overwrite) every file in the dump.
      const keepSet = new Set<string>();
      for (const [rel, text] of dump) {
        const abs = join(allodDir, rel);
        keepSet.add(abs);
        await mkdir(dirname(abs), { recursive: true });
        await writeFile(abs, text, "utf8");
      }

      // Prune files no longer in the dump (e.g. removed proposals).
      const existing = collectFiles(allodDir);
      for (const abs of existing) {
        if (!keepSet.has(abs)) {
          await unlink(abs);
        }
      }
    },
  };
}
