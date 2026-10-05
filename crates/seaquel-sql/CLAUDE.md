# seaquel-sql

Moved from the root `CLAUDE.md`, which has the overview and the crate map. "Above" and "below" may point at sections that now live in another `CLAUDE.md`.

- `seaquel-sql` — pure SQL text work, no I/O: one hand scanner that follows each engine's quoting (`scan.rs`), with statement splitting, statement at cursor and the row-limit check on top; `statements.rs` (query type, the destructive-statement check, the source table for inline editing, `change_summary` for the pending-changes sheet's descriptions); `read_only.rs` (the AI's read-only check); `params.rs` (`{{param}}` substitution); `create_table.rs` (the table editor's SQL pane); and `ast/`, sqlparser-rs used only where an AST is needed (query builder and tutorial `ParsedQuery`, the Visual tab, column sources). It works in UTF-8 byte offsets. Parity fixtures in `crates/seaquel-sql/tests/fixtures` are frozen (see its README). Nothing in it may panic on user input: in the browser a panic is a trap.
