-- A migration only the tests use (`Storage::open_with_migrator`). It fails if
-- it runs twice on one file (no IF NOT EXISTS), and the insert keeps the
-- winner busy long enough for a second open to reach the migrator.
CREATE TABLE race_marker (id INTEGER PRIMARY KEY);
INSERT INTO race_marker DEFAULT VALUES;
CREATE TABLE race_filler (x INTEGER);
WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 200000)
INSERT INTO race_filler (x) SELECT x FROM c;
