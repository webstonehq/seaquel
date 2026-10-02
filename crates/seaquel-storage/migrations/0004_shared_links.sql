-- Phase 5e (Decision 33 of the 5e plan): where each shared row's file is,
-- and the content both sides had at the last sync.
--
-- `projects.shared_dir` is the project's directory under
-- `.seaquel/projects/` in its repo (`git_repo_path`), so the link survives
-- a rename of the local project.
--
-- `saved_queries.shared_path` and `dashboards.shared_path` are the
-- repo-relative path of the row's file. A connection's template path stays
-- in `shared_connection_id` (`<repoId>:<path>`), where older releases look
-- for it.
--
-- `shared_base` is the hash of the content both sides had at the last
-- sync, so Core tells a local change from a teammate's. `shared_file_id`
-- is the `id` Core writes into the file (Q22).
--
-- NULL means "not known": the project's directory is the slug of its
-- name, a row's file is the slug path of its name, and there is no base.
-- No data step: the first sync fills them from what it pairs.
--
-- Expand-only (see README.md): nullable columns with no default and two
-- indexes. Older releases never read the columns; their upserts name the
-- columns they know, so a link they don't know stays as it was, and the
-- next sync sees their change as a row change. It names every column, so
-- it works on files that started on v2026.4.5-beta.1, where
-- `saved_queries.project_id` and `dashboards.project_id` are nullable,
-- last and have no foreign key.

ALTER TABLE projects ADD COLUMN shared_dir TEXT;

ALTER TABLE saved_queries ADD COLUMN shared_path TEXT;
ALTER TABLE saved_queries ADD COLUMN shared_base TEXT;
ALTER TABLE saved_queries ADD COLUMN shared_file_id TEXT;

ALTER TABLE dashboards ADD COLUMN shared_path TEXT;
ALTER TABLE dashboards ADD COLUMN shared_base TEXT;
ALTER TABLE dashboards ADD COLUMN shared_file_id TEXT;

ALTER TABLE connections ADD COLUMN shared_base TEXT;
ALTER TABLE connections ADD COLUMN shared_file_id TEXT;

CREATE INDEX IF NOT EXISTS idx_saved_queries_shared_path ON saved_queries(project_id, shared_path);
CREATE INDEX IF NOT EXISTS idx_dashboards_shared_path ON dashboards(project_id, shared_path);
