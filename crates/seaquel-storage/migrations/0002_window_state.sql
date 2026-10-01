-- Phase 5d-2 (Decision 22 of the 5d plan): view state per window, plus
-- the dashboards' stored name key (Decision 21) and an index the
-- re-survey found missing.
--
-- `windows` holds one row per desktop window (its webview label) or web
-- browser tab (`win-<uuid>`), with the project it shows. The user is the
-- file (a web user's `meta.db` holds only their windows), so there is no
-- user column. `active_project_id` has no foreign key: a removed project
-- just falls back.
--
-- `window_state` holds one window's view of one project: its open tabs
-- (with their text), pane layout and active ids, as one JSON blob Core
-- writes and reads byte for byte. `rev` is the page's save counter; a save
-- lands only when its `rev` is higher than the stored one. Both prunes
-- (windows unused for 30 days, and the counts) are indexed, `LIMIT`-bounded
-- deletes on `updated_at`.
--
-- Every view-state save also writes the legacy `project_state` and `tabs`
-- rows, so older releases keep seeing the most recently saved window's
-- tabs; nothing here changes those tables.
--
-- `saved_canvases(project_id)`: saved workflows are listed and removed per
-- project, which scanned the table.
--
-- `ai_messages(chat_id, timestamp)`: a chat's messages are read in
-- `timestamp, rowid` order, which `idx_ai_messages_chat(chat_id)` alone
-- could only give by sorting every row it read. (The older index stays:
-- migrations are expand-only.)
--
-- `dashboards.name_key`, its index and stale-key trigger follow
-- `0001_name_keys.sql` exactly: storage writes the key with every name it
-- stores, the `backfill_dashboard_name_keys` data step fills the rows that
-- were here before, and the trigger NULLs the key when an older release
-- renames a dashboard.
--
-- Expand-only (see README.md): new tables, a nullable column with no
-- default, indexes and a trigger. It names every column, so it works on
-- files that started on v2026.4.5-beta.1, where `dashboards.project_id` is
-- nullable, last and has no foreign key.

CREATE TABLE IF NOT EXISTS windows (
    window_id TEXT PRIMARY KEY,
    active_project_id TEXT,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_windows_updated ON windows(updated_at);

CREATE TABLE IF NOT EXISTS window_state (
    window_id TEXT NOT NULL REFERENCES windows(window_id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    state TEXT NOT NULL,
    rev INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (window_id, project_id)
);
CREATE INDEX IF NOT EXISTS idx_window_state_project ON window_state(project_id, updated_at DESC);

CREATE INDEX IF NOT EXISTS idx_saved_canvases_project ON saved_canvases(project_id);
CREATE INDEX IF NOT EXISTS idx_ai_messages_chat_time ON ai_messages(chat_id, timestamp);

ALTER TABLE dashboards ADD COLUMN name_key TEXT;
CREATE INDEX IF NOT EXISTS idx_dashboards_name_key ON dashboards(project_id, name_key);

CREATE TRIGGER IF NOT EXISTS dashboards_name_key_stale
AFTER UPDATE OF name ON dashboards
WHEN NEW.name IS NOT OLD.name AND NEW.name_key IS OLD.name_key
BEGIN
    UPDATE dashboards SET name_key = NULL WHERE rowid = NEW.rowid;
END;
