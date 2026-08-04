/**
 * @allod/core — public entry point.
 *
 * Re-exports the WASM class and the Node.js persistence backend.
 * The WASM module must be built first: `pnpm build`
 */

export { AllodGraph } from "../pkg/allod_wasm.js";
export { fsBackend } from "./store.js";
export type { FsBackend } from "./store.js";
