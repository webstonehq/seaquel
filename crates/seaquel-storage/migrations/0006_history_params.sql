-- Cleanup pass B: an applied grid edit keeps its values in query history.
--
-- `query_history.params` is the JSON array of values the change was bound
-- with, in the cell wire format (`seaquel_types::Value`: plain JSON, or
-- `{"$sq": kind, "v": ...}` for bigint, NaN/inf, decimal, bytes and json).
-- The row's `query` stays the SQL Core ran, with its `$n`/`?`/`@Pn`
-- placeholders. NULL means "no values": a run (which records its text with
-- `{{param}}`s), a change without binds, or a row written before this
-- migration or by an older release.
--
-- Expand-only (see README.md): one nullable column with no default. Older
-- releases never read it (their loads read columns by name), and their
-- appends name their own columns, so a row they write gets NULL.

ALTER TABLE query_history ADD COLUMN params TEXT;
