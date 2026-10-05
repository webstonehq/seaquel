# seaquel-wasm

Moved from the root `CLAUDE.md`, which has the overview and the crate map. "Above" and "below" may point at sections that now live in another `CLAUDE.md`.

- `seaquel-wasm` — wasm-bindgen glue over `seaquel-sql`, loaded by the Svelte app on desktop, web and the demo. Strings and JSON in and out, and every position that crosses is a UTF-16 offset (`utf16_to_byte` and friends live in `seaquel_sql::offsets`, re-exported by `offsets.rs`, so Core's `db.run` picks the same statement for a cursor as the editor). The root `src/routes/+layout.ts` awaits `initSeaquelWasm` before anything renders, so calls are synchronous. Linked with a 2 MB stack (`build.rs`).
