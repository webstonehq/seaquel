-- Phase 5d-2 Task 7 probe fixes (5d plan, "Probe fixes (as built)").
--
-- 1. Which window saved last. `windows` and `window_state` broke ties
-- between writes in the same millisecond by rowid, one table ascending and
-- the other descending, and an upsert keeps its row's rowid, so neither
-- told which write came last: `windowGet`, a new window's copy and the
-- legacy mirror (written by every save) could disagree. Each write now
-- records `write_seq`, one past the highest in its table (`windows`) or
-- its project (`window_state`), taken inside the write's transaction, so
-- "most recent" is the last write committed. Rows already here are
-- numbered in the order the old queries read them (`updated_at`, then the
-- old tie-break), so nothing moves.
--
-- 2. Lists without bodies. `workflowsList` and `dashboardVersionsList`
-- answer metadata only, and must not parse every stored body to get it.
-- `saved_canvases.meta` holds a workflow's `{name, createdAt, updatedAt}`
-- as JSON, written with its data; `dashboard_versions.widget_count` holds
-- the length of a snapshot's `widgets`. A body that can't be read is
-- marked, not left NULL: `meta` `'null'` (no list shows the row) and
-- `widget_count` `-1` (no count). NULL means "not known yet": a row an
-- older release wrote (its replace-all save deletes and inserts whole
-- `saved_canvases` rows, without `meta`), which the lists compute from
-- that row's body, and which every writable open fills again
-- (`refill_list_meta`), finding them through the two partial indexes. The
-- trigger clears `meta` if anything updates `data` without it.
--
-- A JSON function is only ever called on text `json_valid` accepted (a
-- `CASE` guarantees the order), so a row that isn't JSON is left NULL
-- instead of failing the migration.
--
-- Expand-only (see README.md): new columns (NOT NULL only with a default),
-- indexes and a trigger, and set-based fills of the new columns. It names
-- every column, so it works on files that started on v2026.4.5-beta.1.

ALTER TABLE windows ADD COLUMN write_seq INTEGER NOT NULL DEFAULT 0;
UPDATE windows SET write_seq = (
    SELECT COUNT(*) FROM windows w
    WHERE w.updated_at < windows.updated_at
       OR (w.updated_at = windows.updated_at AND w.rowid <= windows.rowid)
);
CREATE INDEX IF NOT EXISTS idx_windows_write_seq ON windows(write_seq);

ALTER TABLE window_state ADD COLUMN write_seq INTEGER NOT NULL DEFAULT 0;
UPDATE window_state SET write_seq = (
    SELECT COUNT(*) FROM window_state s
    WHERE s.project_id = window_state.project_id
      AND (s.updated_at < window_state.updated_at
           OR (s.updated_at = window_state.updated_at AND s.rowid >= window_state.rowid))
);
CREATE INDEX IF NOT EXISTS idx_window_state_project_seq ON window_state(project_id, write_seq);

ALTER TABLE saved_canvases ADD COLUMN meta TEXT;
UPDATE saved_canvases SET meta = COALESCE(
    CASE WHEN typeof(data) = 'text' AND json_valid(data) THEN
        CASE json_type(data) WHEN 'null' THEN NULL WHEN 'object' THEN json_object(
            'name', CASE WHEN json_type(data, '$.name') = 'text' THEN data ->> '$.name' END,
            'createdAt', CASE WHEN json_type(data, '$.createdAt') = 'text'
                THEN data ->> '$.createdAt' END,
            'updatedAt', CASE WHEN json_type(data, '$.updatedAt') = 'text'
                THEN data ->> '$.updatedAt' END)
        ELSE '{}' END END,
    'null');
CREATE INDEX IF NOT EXISTS idx_saved_canvases_meta_pending ON saved_canvases(project_id)
    WHERE meta IS NULL;
CREATE TRIGGER IF NOT EXISTS saved_canvases_meta_stale
AFTER UPDATE OF data ON saved_canvases
WHEN NEW.data IS NOT OLD.data AND NEW.meta IS OLD.meta
BEGIN
    UPDATE saved_canvases SET meta = NULL WHERE rowid = NEW.rowid;
END;

ALTER TABLE dashboard_versions ADD COLUMN widget_count INTEGER;
UPDATE dashboard_versions SET widget_count = COALESCE(
    CASE WHEN typeof(snapshot) = 'text' AND json_valid(snapshot) THEN
        CASE WHEN json_type(snapshot, '$.widgets') = 'array'
            THEN json_array_length(snapshot, '$.widgets') END END,
    -1);
CREATE INDEX IF NOT EXISTS idx_dashboard_versions_count_pending
    ON dashboard_versions(dashboard_id) WHERE widget_count IS NULL;
