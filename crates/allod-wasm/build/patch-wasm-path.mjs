// wasm-pack's nodejs loader reads allod_wasm_bg.wasm from __dirname, which
// bundlers (e.g. Bun --compile) bake into a build-machine absolute path.
// Patch the loader to honor ALLOD_WASM_PATH so embedders can point it at a
// file shipped next to their binary.
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const file = join(dirname(fileURLToPath(import.meta.url)), "..", "pkg", "allod_wasm.js");
const src = readFileSync(file, "utf8");
const needle = "const wasmPath = `${__dirname}/allod_wasm_bg.wasm`;";
const replacement =
  "const wasmPath = process.env.ALLOD_WASM_PATH || `${__dirname}/allod_wasm_bg.wasm`;";

if (src.includes(replacement)) {
  console.log("[patch-wasm-path] already patched");
} else if (src.includes(needle)) {
  writeFileSync(file, src.replace(needle, replacement));
  console.log("[patch-wasm-path] patched wasm path to honor ALLOD_WASM_PATH");
} else {
  console.error("[patch-wasm-path] ERROR: expected loader line not found — wasm-pack output changed");
  process.exit(1);
}
