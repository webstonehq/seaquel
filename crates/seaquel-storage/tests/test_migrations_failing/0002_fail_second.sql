-- Test-only (`tests/open.rs`): fails after creating a table, on a table that
-- doesn't exist.
CREATE TABLE fail_second (x INTEGER);
INSERT INTO no_such_table (x) VALUES (1);
