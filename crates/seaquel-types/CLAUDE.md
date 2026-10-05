# seaquel-types

Moved from the root `CLAUDE.md`, which has the overview and the crate map. "Above" and "below" may point at sections that now live in another `CLAUDE.md`.

- `seaquel-types` — wire types, including the dialect types (`SchemaTable`, `ExplainResult`, `CreateTableDefinition`, …), `Value`, the metadata row types (`storage.rs`, the `Persisted*` types) and `names.rs` (`name_key`, shared by Core's duplicate check and storage's `name_key` columns). `npm run types:gen` regenerates `src/lib/types/generated/` from `seaquel-types`, `seaquel-rpc`, `seaquel-sql`, `seaquel-wasm` and `seaquel-workspace` (the run, edit, library, state, shared, import and AI types); never edit those by hand.

- `ConnectConfig` also has `tls_server_name` (MSSQL: the name the certificate is checked against while the socket goes to `host`, set through an SSH tunnel) and `duckdb_config` (DuckDB open options from `duckdb://path?key=value`; `restricted` wins over them).
