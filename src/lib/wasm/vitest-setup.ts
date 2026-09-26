// Loads seaquel-wasm for every vitest file, so code that calls it
// synchronously works in tests as it does in the app. About 2 ms per file.
import { existsSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { initSeaquelWasmSync } from "./index";

const wasmPath = fileURLToPath(new URL("./pkg/seaquel_wasm_bg.wasm", import.meta.url));
if (!existsSync(wasmPath)) {
  throw new Error(`${wasmPath} is missing: run npm run wasm:build first`);
}
initSeaquelWasmSync(readFileSync(wasmPath));
