# seaquel-engine-sqlite

Moved from the root `CLAUDE.md`, which has the overview and the crate map. "Above" and "below" may point at sections that now live in another `CLAUDE.md`. The shared engine rules are in `crates/seaquel-engine/CLAUDE.md`.

- **SQLite:** cells decode by storage class (`typeof`), not declared type; BLOBs are bytes. SQLite has no `DEFAULT` in `UPDATE`, so Set to default writes the column's default expression, which Core reads from the table's metadata when it plans the edit (none: NULL). Edits SQLite can't make come back from `alterTable` as `-- …` note lines; the table editor shows them (`splitDdlScript` in `src/lib/utils/ddl-script.ts`).
