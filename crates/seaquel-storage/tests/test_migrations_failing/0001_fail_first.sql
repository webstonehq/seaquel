-- Test-only (`tests/open.rs`): succeeds, and must be rolled back when 0002
-- fails in the same open.
CREATE TABLE fail_first (x INTEGER);
INSERT INTO fail_first (x) VALUES (1);
