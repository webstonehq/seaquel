// The demo's browser module (crates/seaquel-browser), built into
// src/lib/wasm/browser-pkg/ only by the demo's scripts (build-wasm.mjs
// --module browser). Desktop and web never build it, so their type check
// can't read its generated .d.ts; TypeScript takes this declaration instead.
// `$lib/core/browser` types the exports it uses as `BrowserModule`.
declare module "$lib/wasm/browser-pkg/seaquel_browser.js" {
  /** Fetches and instantiates the module. */
  export default function init(options?: {
    module_or_path?:
      | RequestInfo
      | URL
      | Response
      | BufferSource
      | WebAssembly.Module
      | Promise<Response>;
  }): Promise<unknown>;
}
