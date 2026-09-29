-- Phase 5d-1 probe fix: a stored name key for connections, projects and
-- saved queries, so Core's duplicate-name check (NAME_TAKEN) is an indexed
-- lookup instead of a scan that folds every name in the project.
--
-- `name_key` holds `seaquel_types::names::name_key(name)`, or NULL when it
-- isn't known. Storage writes it with the name (`connections::insert`, …),
-- and the `backfill_name_keys` data step fills the rows that were here
-- before. NULL is always safe: the check also reads the rows whose key is
-- NULL and folds their names itself.
--
-- Expand-only (see README.md): nullable columns with no default, indexes
-- and triggers. Older releases write rows without a key; the triggers keep
-- them honest when those releases rename a row, by setting the key to NULL
-- whenever `name` changes and `name_key` doesn't. (A rename that keeps its
-- key, a case-only one through Core, is NULLed too; storage sets the key
-- again right after.)

ALTER TABLE connections ADD COLUMN name_key TEXT;
ALTER TABLE projects ADD COLUMN name_key TEXT;
ALTER TABLE saved_queries ADD COLUMN name_key TEXT;

CREATE INDEX IF NOT EXISTS idx_connections_name_key ON connections(project_id, name_key);
CREATE INDEX IF NOT EXISTS idx_projects_name_key ON projects(name_key);
CREATE INDEX IF NOT EXISTS idx_saved_queries_name_key
    ON saved_queries(project_id, COALESCE(folder, ''), name_key);

CREATE TRIGGER IF NOT EXISTS connections_name_key_stale
AFTER UPDATE OF name ON connections
WHEN NEW.name IS NOT OLD.name AND NEW.name_key IS OLD.name_key
BEGIN
    UPDATE connections SET name_key = NULL WHERE rowid = NEW.rowid;
END;

CREATE TRIGGER IF NOT EXISTS projects_name_key_stale
AFTER UPDATE OF name ON projects
WHEN NEW.name IS NOT OLD.name AND NEW.name_key IS OLD.name_key
BEGIN
    UPDATE projects SET name_key = NULL WHERE rowid = NEW.rowid;
END;

CREATE TRIGGER IF NOT EXISTS saved_queries_name_key_stale
AFTER UPDATE OF name ON saved_queries
WHEN NEW.name IS NOT OLD.name AND NEW.name_key IS OLD.name_key
BEGIN
    UPDATE saved_queries SET name_key = NULL WHERE rowid = NEW.rowid;
END;
