# seaquel-engine-mysql

Moved from the root `CLAUDE.md`, which has the overview and the crate map. "Above" and "below" may point at sections that now live in another `CLAUDE.md`. The shared engine rules are in `crates/seaquel-engine/CLAUDE.md`.

- **MySQL/MariaDB TIMESTAMP values are shown as UTC wall-clock time**: sqlx sets the session `time_zone` to `'+00:00'`. Don't add a `timezone` to connection strings; in a zone with DST two instants print the same and a TIMESTAMP key would match the wrong row.
