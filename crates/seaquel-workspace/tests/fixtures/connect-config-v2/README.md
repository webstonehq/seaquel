# connect-config fixtures, v2

The JSON files here were reformatted to one case per line on 2026-10-04, with no value changed.

`docs/plans/` (the plans, and the recorders' copies in `docs/plans/artifacts/`) was deleted on 2026-10-04. The paths under it named below are in git history: `git show ae7f269:<path>`.

The v1 files (`../connect-config`) and the test comparing v2 with them (the diff report below) were deleted on 2026-10-04; git history keeps them. Each case's `changedBy`, `v1` and `v1Notes` stay as the record of what changed.

These files say what Core's connection builder should produce once phase 5a fixes the recorded quirks (the phase 5a plan, Decision 6). Every case in `../connect-config` is here under the same id: the 107 saved-row cases from phase 4 and the 53 form cases (`form-add.json`, `form-test.json`) from phase 5a, Task 1. 160 in all.

**v2 is written, not recorded.** Each case keeps its v1 input, except that a form's SSL mode that came only from the wizard's old `disable` default is now unset (settled choice I, below). Its expected output is v1's unless a row of Decision 6 changes it, and then `changedBy` names the rows. The owner settled three open choices on 2026-10-01 (A, B and I); they are folded into the rows below and listed under "Settled choices". There is no TypeScript that behaves this way, so nothing can regenerate these files. Edit them by hand, and only when the intended behaviour changes. Say why under "Changes" at the end.

**How they're used.**
- Phase 5a, Task 2 replays every case through `Workspace::connect` or `Workspace::test`.
- The frozen v1 files were a diff report: a case had to differ from v1 exactly when its `changedBy` was non-empty, and the ids matched v1 one to one. `tests/connect_config.rs` checked that until v1 was deleted (above).

`Saved` and `Form` go through one builder, so a saved row and a form that describe the same connection now get the same config. v1's disagreements between autoReconnect and the reconnect tab are gone. v1 compared against `case[case.gui]`, and so does v2.

## A case

```jsonc
{
  "name": "pg/stored-string-default-port", // the v1 id
  "target": "saved" | "form",   // the ConnectTarget to build: Saved { id } or Form { form }
  "op": "connect" | "test",     // Workspace::connect or Workspace::test
  "changedBy": ["2"],           // Decision 6 rows ("1"…"11", "7a"…"7d", "5-wizard") that make `expected` differ from v1; [] = same as v1
  "needsLiveCheck": true,       // optional: Task 2 confirms it against a real server
  "notes": "…",                 // optional: why v2 expects this
  "v1Notes": "…",               // optional: the v1 case's own notes
  "input": {
    // target "saved":
    "row": { … },               // PersistedConnection, as in v1
    "secrets": { "db:<id>": "…" }, // the workspace's SecretStore
    "secretErrors": ["db:<id>"],   // optional: reads that fail
    "supplied": {},             // SuppliedSecrets from the caller: none in these cases
    // target "form":
    "form": { … },              // ConnectionFormData without password, sshPassword, sshKeyPassphrase;
                                // no sslMode = the wizard's "Default" (unset)
    "supplied": { "db": "…", "ssh": "…", "sshKey": "…" }, // those three, when non-empty
    // both:
    "createIfMissing": false,
    "tunnelPort": 50001         // the local port the tunnel reports; null without SSH
  },
  "expected": {
    "secretsRead": ["db:<id>"], // saved only: the store keys read, in order
    "tunnel": { … },            // optional: the TunnelConfig as JSON (camelCase, trustHostKey absent)
    "config": { … },            // the ConnectConfig as JSON, absent fields left out
    "error": "CREDENTIALS_REQUIRED" // instead of tunnel and config
  },
  "v1": { "file": "connect-config/postgres.json", "output": "autoReconnect" } // or reconnectTab, form, recorded
}
```

- **`ConnectionForm`.** `form` is the tab's `ConnectionFormData`, so the fields have the names the GUI already uses. Task 2 defines `ConnectionForm` and maps these fields onto it.
- **`supplied`.** On a form case it holds the form's three secrets.
- **Secret reads.** A saved case reads the store with the v1 rules (autoReconnect's order and give-ups), because nothing in these cases is supplied. So `secretsRead` equals v1 everywhere.
- **Two new `ConnectConfig` fields**, as Task 2 added them:
  - `tls_server_name` (row 6): the name an MSSQL server's certificate is checked against.
  - `duckdb_config` (row 10): DuckDB options parsed out of the path, as an object of strings.
- **What isn't compared.** Tunnel lifecycle (`tunnelClosed` in v1: `test` closes its tunnel) isn't part of `expected`. Neither is a host-key fingerprint.

The files group cases by engine, as v1 does. Form cases go in their engine's file, and Postgres and MySQL form cases over SSH go in `ssh.json`.

| file            | cases | saved | form | differ from v1 |
| --------------- | ----- | ----- | ---- | -------------- |
| `postgres.json` | 48    | 31    | 17   | 31             |
| `mysql.json`    | 17    | 11    | 6    | 6              |
| `mariadb.json`  | 8     | 4     | 4    | 2              |
| `mssql.json`    | 21    | 14    | 7    | 7              |
| `sqlite.json`   | 15    | 8     | 7    | 3              |
| `duckdb.json`   | 14    | 8     | 6    | 2              |
| `ssh.json`      | 21    | 15    | 6    | 15             |
| `secrets.json`  | 11    | 11    | 0    | 2              |
| `shared.json`   | 5     | 5     | 0    | 0              |
| **total**       | 160   | 107   | 53   | 68             |

## The builder these cases pin

Both targets first become the same thing: a row (type, host, port, database, username, SSL mode, string, tunnel) plus secrets.

1. **Secrets.**
   - `Saved` takes supplied secrets first, then reads the store under the row's save flags, exactly as v1 did. Its username falls back to the string's user, percent-decoded (row 11).
   - `Form` reads nothing: its secrets are the supplied ones, and its save flags don't matter.
2. **Giving up with `CREDENTIALS_REQUIRED`** happens in the same places for both targets:
   - no database password with `savePassword` off. This applies only to `Saved`: an empty form password is a passwordless connection;
   - SSH password auth with no SSH password;
   - SSH key auth with no key path;
   - an empty host, where the host is used: MSSQL, or a string built from the fields;
   - an empty MSSQL username (row 3: SQL Server has no default login).
3. **File engines never tunnel and never read a secret** (row 7d).
4. **Postgres, MySQL and MariaDB** (MariaDB on the `mysql` driver, with `mariadb://`, row 9):
   - **A stored or typed string** goes out as it is (row 4): its scheme (row 2), port, query and IPv6 brackets are kept. One exception (settled B): when it has no `sslmode` (Postgres) or `ssl-mode` (MySQL, MariaDB), a TablePlus `tLSMode` is replaced, in its place, by that parameter: 0 is prefer, 1 disable, 2 require (MySQL spells them `PREFERRED`, `DISABLED`, `REQUIRED`).
   - **A TablePlus `+ssh` URL** is parsed into its database part and its SSH part (row 7b).
   - **An empty string** is built from the fields (row 1), the way `buildConnectionString` does:
     - the type's scheme;
     - percent-encoded user info;
     - no port when it is the default, with port 0 meaning the default (row 5);
     - `/database`;
     - the SSL parameter. Postgres gets `sslmode=<mode>`. MySQL and MariaDB get `ssl-mode=`, mapping `disable` to `DISABLED`, `allow` and `prefer` to `PREFERRED`, `require` to `REQUIRED`, `verify-ca` to `VERIFY_CA` and `verify-full` to `VERIFY_IDENTITY` (row 8). No mode means no parameter, which leaves the driver's default (row 5).
   - **The password**, when there is one (supplied, or saved under the flags), then goes into the user info, `encodeURIComponent`-encoded, replacing any password the string already has (settled A). That includes an empty user name, which gives `://:pw@` (row 3). A key=value string gets its `Password=`/`Pwd=` pair replaced instead; no engine here sends one (MSSQL ignores the string, row 7a), so no case pins it.
   - **Through a tunnel**, a URL parser sets the host to `127.0.0.1` and the port to the tunnel's, keeping everything else.
5. **The tunnel** forwards to the host and port the connection uses (row 7c):
   - with a string, the string's host and port (the engine's default port when it has none);
   - otherwise, and always for MSSQL, the fields.

   Empty `password`, `keyPath` and `keyPassphrase` are left out (row 5). The SSH password is sent whenever there is one, even with key auth (as v1's `secrets/all-flags-all-secrets`).
6. **MSSQL** takes the fields and ignores the string (row 7a).
   - Port 0 means 1433 (row 5).
   - `encrypt` unless the mode is `disable`. `trust_cert` only for no mode, `disable`, `allow` or `prefer`. So no mode means encrypt and trust, as autoReconnect did (row 5).
   - Through a tunnel: `host` `127.0.0.1`, the tunnel's port, and `tls_server_name` set to the row's host (row 6).
7. **SQLite:**
   - the string as it is, `?mode=ro` included (row 10), or else `sqlite://<databaseName>`;
   - `create_if_missing` from the request.
8. **DuckDB:**
   - the string without `duckdb://` or `duckdb:`, with its query parsed out into `duckdb_config` (row 10);
   - `:memory:` when that leaves nothing (`duckdb://`);
   - with no string, the database name, or `:memory:` when it is empty.

## Each row of Decision 6

"Changes" counts the cases whose `changedBy` names the row; a case can name several rows. "Also governs" counts cases the row applies to without itself changing their output. Another row may still change them, as the sections below say.

| row | quirk (v1)                                     | changes | also governs |
| --- | ---------------------------------------------- | ------- | ------------ |
| 1   | no stored string: autoReconnect can't connect  | 0       | 23                       |
| 2   | reinjection turns `postgresql://` into `postgres://` | 33 | —                        |
| 3   | empty username with a password                 | 2       | 4 (checked live)         |
| 4   | strings rebuilt from the fields                | 17      | —                        |
| 5   | tab and form defaults                          | 15      | 1                        |
| 5-wizard | the wizard's `disable` default (settled I) | 4      | —                        |
| 6   | MSSQL TLS over SSH                             | 4       | — (checked live)         |
| 7a  | key=value MSSQL string over SSH                | 0       | 2                        |
| 7b  | TablePlus `+ssh` URL                           | 2       | —                        |
| 7c  | tunnel forwards to the row, not the string     | 1       | —                        |
| 7d  | a tunnel for SQLite                            | 1       | 1                        |
| 8   | MySQL `verify-*` unmapped                      | 4       | —                        |
| 9   | MariaDB through the mysql driver               | 0       | 8 (checked live)         |
| 10  | file-engine paths and parameters               | 4       | 5                        |
| 11  | the URL username is decoded                    | 0       | 3                        |

68 cases differ from v1 and 92 equal it. The 18 that needed a live check were checked in Task 2 (below); none is left.

### Row 1: build the string from the fields

**Changes nothing.** v1 already expected the reconnect tab's rebuild for these rows (`gui: "reconnectTab"`), because autoReconnect couldn't connect them. Now `Saved` builds that same string itself. The rows are the 23 sqlx-engine rows with no stored string, which each note:
- `pg/sslmode-{disable,allow,prefer,require,verify-ca,verify-full}`, `pg/no-sslmode`, `pg/empty-username-no-string`, `pg/url-special-password-rebuilt`, `pg/plus-amp-equals-space-password-rebuilt`, `pg/unicode-user-password-rebuilt`;
- `mysql/sslmode-{disable,allow,prefer,require,verify-ca,verify-full}`, `mariadb/no-string-require`, `secrets/mysql-save-on-missing-no-string`;
- `sqlite/no-string`, `shared/sqlite-imported`;
- `ssh/pg-no-string`, `shared/pg-imported-then-saved`.

Four of them differ for other reasons: `pg/no-sslmode` and `ssh/pg-no-string` under row 5, and `mysql/sslmode-verify-ca` and `-verify-full` under row 8.

### Row 2: keep the scheme the user stored (33 cases)

v1 turned `postgresql://` into `postgres://` whenever it touched the string: when it reinjected a password or rewrote the host for a tunnel, and in `getConnectionData` for a typed one. v2 keeps `postgresql://`. A string that was already `postgres://` doesn't change (`pg/string-postgres-scheme`), and nor does a `postgresql://` string with no password and no tunnel (`secrets/pg-save-on-missing`, `-save-on-empty`, `-read-error`).
- **The scheme is the only change** in 25 saved cases: `pg/stored-string-default-port`, `pg/stored-string-no-port`, `pg/custom-port`, `pg/string-with-password-saved-differs` (settled A keeps v1's `new-pw`), `pg/string-params-kept`, `pg/string-port-differs-from-row`, `pg/empty-username`, `pg/username-from-url`, `pg/url-special-password`, `pg/percent-in-password`, `pg/plus-amp-equals-space-password`, `pg/unicode-user-password`, `pg/unicode-user-from-url`, `pg/at-in-username`, `pg/ipv6-host`, `pg/localhost`, `ssh/pg-password-auth`, `ssh/pg-key-with-passphrase`, `ssh/pg-key-no-passphrase`, `ssh/pg-key-passphrase-flag-but-missing`, `ssh/pg-disabled-tunnel`, `ssh/pg-null-tunnel`, `ssh/pg-special-password`, `secrets/all-flags-all-secrets` and `secrets/ssh-flags-without-tunnel`.
- **With other rows:** `pg/tableplus-url` (row 4, the `tLSMode` translation) and `ssh/pg-string-port-differs` (row 7c).
- **Form cases**, which v1 rebuilt as `postgres://` (with row 4): `add/pg-paste-default-port`, `add/pg-paste-params`, `add/pg-paste-tableplus`, `add/pg-paste-tableplus-tls1`, `add/pg-paste-tableplus-tls2` and `test/pg-paste-params`.

### Row 3: an empty username with a password (2 cases, 4 more checked live)

v2 keeps `://:pw@` for the sqlx engines, and refuses an empty MSSQL username.
- **`add/pg-empty-username-with-password`.** `buildConnectionString` writes no user info without a username, and the form path doesn't reinject, so v1 dropped the password. v2 sends `postgres://:pw@db.example.com/app?sslmode=disable`.
- **`mssql/empty-username-no-string`** is now `CREDENTIALS_REQUIRED`: SQL Server rejects an empty login (checked live, below), so Core refuses before connecting, with a message naming the username.
- **Unchanged, checked live:** `pg/empty-username` (it differs, by row 2 only), `pg/empty-username-no-string`, `mysql/empty-username` and `shared/pg-imported-then-saved`. sqlx accepts the empty user name and uses its own default: `PGUSER` or the client's OS user for Postgres, `root` for MySQL and MariaDB. That's the client's default user, not the server's, as the plan's table put it.

### Row 4: use the string as it is (17 cases)

The rule: rebuild only an empty string. The password still replaces the string's (settled A), and TablePlus `tLSMode` becomes the SSL parameter (settled B).
- `pg/tableplus-url` (saved): v1's autoReconnect passed `tLSMode=0` through. v2 puts `sslmode=prefer` in its place and keeps the other TablePlus parameters (with row 2).
- **Typed strings v1 rebuilt**, because they didn't have exactly three `:` parts or were TablePlus URLs:
  - `add/pg-paste-default-port` (v1 added `sslmode=disable` from the form);
  - `add/pg-paste-params` and `test/pg-paste-params` (v1 dropped `application_name`);
  - `add/pg-paste-ipv6` (v1 dropped the port);
  - `add/pg-paste-tableplus`, `add/pg-paste-tableplus-tls1` and `add/pg-paste-tableplus-tls2` (v1 dropped the TablePlus parameters; v2 keeps them and turns `tLSMode` 0, 1 and 2 into `sslmode=prefer`, `disable` and `require`);
  - `add/mysql-paste-tableplus-tls2` (`tLSMode=2` becomes `ssl-mode=REQUIRED`; v1 also dropped `name`);
  - `add/pg-paste-unicode-user`;
  - `add/mysql-paste-params` (v1 dropped `charset`);
  - `add/mariadb-paste` (v1 added `ssl-mode=DISABLED`);
  - `add/sqlite-paste-mode-ro` and `test/sqlite-paste-mode-ro`, and `add/duckdb-paste-params` (v1 dropped the parameters; with row 10);
  - `add/pg-paste-tableplus-ssh` (with rows 5 and 7b).
- `add/pg-paste-no-password-then-typed`: the paste has no password and the user typed one. v1 sent the string without it; v2 puts it in.
- **The form's SSL mode doesn't apply to a typed string** in v2, just as a stored row's didn't on autoReconnect.
- `pg/string-with-password-saved-differs` isn't here any more: under settled A the keychain's `new-pw` replaces the string's `old-pw`, as v1 did, so only row 2 changes it.

### Row 5: no tab defaults (15 cases, 1 more covered)

- **A missing SSL mode is the driver's default.** `pg/no-sslmode` loses v1's `sslmode=disable`. MSSQL with no mode keeps encrypt and trust (`mssql/no-sslmode`, unchanged). Forms get there through 5-wizard, below.
- **Port 0 is the engine's default port.**
  - `add/pg-port-zero`: the port is left out of the string. v1 sent `:0` (with 5-wizard).
  - `test/mssql-port-zero`: 1433.
- **An empty host is an error.** `test/pg-empty-host` is `CREDENTIALS_REQUIRED`; v1 sent `postgres://alice:pw@/app` (see "Choices" E).
- **An empty tunnel field means none.** `password`, `keyPath` and `keyPassphrase` are no longer sent as `""`:
  - saved cases that v1 took from the tab: `ssh/pg-no-string` and `mssql/key-value-string-ssh` (with row 6);
  - form cases: `add/pg-ssh-password`, `add/pg-ssh-key` and `add/mysql-ssh-key` (both with 5-wizard), `add/mssql-ssh-require` (with row 6), `add/pg-paste-tableplus-ssh` (with rows 4 and 7b), `test/pg-ssh-password`, `test/mariadb-ssh-password` and `test/mssql-ssh-key-verify-full` (with row 6).
- **One builder means `Saved`'s SSH rule applies to a form.** In `test/pg-ssh-password-empty`, password auth has an empty SSH password. That is now `CREDENTIALS_REQUIRED` before any tunnel opens; v1 asked for a tunnel with password `""` (see "Choices" F).

### 5-wizard: the wizard's SSL mode defaults to unset (4 cases)

Settled choice I. The wizard's SSL dropdown gets a "Default" option, and it becomes the initial value (Task 6), so a form whose user never picked a mode sends none, and row 5's defaults apply: SQL Server encrypts and trusts any certificate, and Postgres and MySQL get no SSL parameter (the driver's default, prefer).
- **Inputs.** Every form case whose `disable` came only from the old default has no `sslMode` in `input.form`: forms filled in field by field without picking a mode, and pastes with no SSL parameter. Cases that picked `disable` on purpose keep it: `add/pg-fields-default-port`, `add/pg-empty-username-with-password`, `add/pg-ssh-password`, `test/pg-empty-host`, `add/pg-paste-tableplus-tls1` (from `tLSMode=1`), `add/mysql-fields-default-port`, `add/mariadb-fields-disable` (added for this) and `add/mssql-fields-disable`.
- **Outputs that change:**
  - `add/mssql-paste-url`: `encrypt` and `trust_cert` true. v1 kept `disable` and connected unencrypted.
  - `add/pg-port-zero`, `add/pg-ssh-key` and `add/mysql-ssh-key`: built from the fields, so the string loses `sslmode=disable` / `ssl-mode=DISABLED` (all three also row 5).
- **Inputs that change, outputs that don't:** the other pastes use their string as it is (`add/pg-paste-default-port`, `add/pg-paste-ipv6`, `add/pg-paste-unicode-user`, `add/pg-paste-tableplus-ssh`, `add/mariadb-paste`), and SQLite and DuckDB ignore the mode. Their `changedBy` doesn't name 5-wizard.
- **Saved rows keep the mode they stored.** No saved case changes.

### Row 6: MSSQL's TLS server name over SSH (4 cases, checked live)

Through a tunnel the config's `host` is `127.0.0.1`, so the certificate used to be checked against that. v2 adds `tls_server_name` with the row's host (`sql.internal`) to `mssql/ssh`, `mssql/key-value-string-ssh`, `add/mssql-ssh-require` and `test/mssql-ssh-key-verify-full`.
- **It's set whenever MSSQL goes through a tunnel**, whatever the mode. With a trusted certificate it changes nothing.
- **tiberius 0.12.3 looks feasible.** Its TLS name is `Config::host` (`native_tls`/`opentls` call `connect(config.get_host(), …)`, and rustls uses `ServerName::try_from(get_host())`). The caller opens the TCP stream itself, so Core can set `host` to the row's host and connect the socket to the tunnel.
- **Checked live in Task 2** (below): tiberius takes it, the socket goes to the tunnel, and the certificate is checked as the server's name. The field stays.

### Row 7a: a key=value MSSQL string over SSH (no changes)

MSSQL uses the fields, so nothing parses the string and the SSH rewrite never touches it.
- `mssql/key-value-string` is unchanged.
- `mssql/key-value-string-ssh` now connects through `Saved` directly; v1 got there only through the tab. Its output differs from v1's by rows 5 and 6 only.

### Row 7b: TablePlus `+ssh` URLs are parsed (2 cases)

- **`pg/tableplus-ssh-url`** stores `postgres+ssh://deploy@bastion.example.com:22/alice@127.0.0.1:5432/app?name=Prod&usePrivateKey=true`.
  - v1's autoReconnect rewrote the outer (SSH) authority and put the database password on the SSH user.
  - v2 parses it. The database part `postgres://alice@127.0.0.1:5432/app` gets the password and goes through the tunnel: `postgres://alice:pw@127.0.0.1:50022/app`. The TablePlus-only `name` and `usePrivateKey` are dropped. The tunnel is the row's own, unchanged from v1 (see "Choices" C).
- **`add/pg-paste-tableplus-ssh`:** the form's tunnel fields came from the same parse. The database part goes through as it is, without the `sslmode=disable` v1's rebuild added (rows 4 and 5).

### Row 7c: the tunnel forwards to the string's host and port (1 case)

`ssh/pg-string-port-differs`: the string says port 6000 and the row 5432. v2's tunnel forwards to `db.internal:6000`; v1 used the row's port. With row 2.

### Row 7d: file engines never tunnel (1 case, 1 more covered)

`add/sqlite-ssh-leftover`: SSH fields left on a SQLite form. v1 opened a tunnel to `localhost:0` and sent `sqlite://127.0.0.1:50106/Users/me/app.db`. v2 opens no tunnel and sends `sqlite:///Users/me/app.db`. `sqlite/ssh-ignored` is unchanged, because autoReconnect already ignored the tunnel.

### Row 8: MySQL `verify-ca` and `verify-full` are mapped (4 cases)

- `verify-ca` becomes `VERIFY_CA`: `mysql/sslmode-verify-ca` and `add/mysql-verify-ca`.
- `verify-full` becomes `VERIFY_IDENTITY`: `mysql/sslmode-verify-full` and `test/mysql-verify-full`.
- v1 sent the lower-case mode, which sqlx-mysql 0.8.6 refuses (`unknown value "verify-ca" for ssl_mode`).

### Row 9: MariaDB keeps `mariadb://` (no changes, 8 checked live)

Every MariaDB case keeps `mariadb://` on the `mysql` driver. Task 2 confirmed live that sqlx accepts the scheme.
- The saved cases: `mariadb/stored-string`, `mariadb/default-port-string`, `mariadb/no-string-require` and `mariadb/ssh`.
- The form cases: `add/mariadb-fields`, `add/mariadb-fields-disable`, `add/mariadb-paste` (row 4) and `test/mariadb-ssh-password` (row 5).

### Row 10: file-engine paths (4 cases, 5 more covered)

- **DuckDB parameters come out of the path.**
  - `duckdb/query-params`: v1 sent them inside it. v2 sends `path` `/Users/me/analytics.duckdb` with `duckdb_config` `{ "access_mode": "read_only" }`.
  - `add/duckdb-paste-params`: v1's rebuild dropped them (row 4).
- **SQLite keeps `?mode=ro`** on both paths. `add/sqlite-paste-mode-ro` and `test/sqlite-paste-mode-ro` change (with row 4). `sqlite/query-params` is unchanged, because autoReconnect kept it.
- **`:memory:`:**
  - `duckdb://` stays `:memory:` (`duckdb/empty-string-path`).
  - An empty DuckDB name is `:memory:` (`duckdb/no-string-no-name`, `test/duckdb-empty-name`).
  - `test/duckdb-paste-memory` stays `:memory:`.

  All four are unchanged.

### Row 11: the URL username is decoded (no changes)

The fallback stays: `pg/username-from-url`, `pg/unicode-user-from-url` and `mssql/username-from-url`. The first two differ from v1 only by row 2.

## Live checks (Task 2, 2026-10-01)

The 18 cases that needed a live check were run against the e2e Docker databases and the SSH container (`crates/seaquel-core/tests/connect.rs`: `row_3_an_empty_user_name_on_each_engine`, `row_6_mssql_over_ssh_checks_the_certificate_as_the_server`, `row_9_the_mariadb_scheme`), with each case's shape pointed at the local servers. Their `needsLiveCheck` flag is gone and their notes say what was seen.

- **Row 3** (`pg/empty-username`, `pg/empty-username-no-string`, `shared/pg-imported-then-saved`, `add/pg-empty-username-with-password`, `mysql/empty-username`, `mssql/empty-username-no-string`):
  - Postgres: `postgres://:pw@127.0.0.1:5432/seaquel_test` parses, and sqlx logs in as its default user, `PGUSER` or the OS user. The server answered `role "m" does not exist` for the OS user `m`. Kept.
  - MySQL and MariaDB: an empty user is `root`. Both connected as root without a password and refused `Access denied for user 'root'@… (using password: YES)` with one. Kept.
  - SQL Server: an empty login fails with 18456, `Login failed for user ''.`, since SQL authentication has no default login. So Core refuses it first (`CREDENTIALS_REQUIRED`, "has no username"), and `mssql/empty-username-no-string` changed to that error (`changedBy` `["3"]`).
- **Row 6** (`mssql/ssh`, `mssql/key-value-string-ssh`, `add/mssql-ssh-require`, `test/mssql-ssh-key-verify-full`): kept.
  - tiberius accepts it. `crates/seaquel-engine-mssql/tests/tls_server_name.rs` connects to a fake server on 127.0.0.1 and reads the TLS ClientHello: its SNI is `tls_server_name` (`sql.internal`), while without the field an IP host sends none.
  - Through the SSH container to `sqlserver:1433`, `prefer` connects. `require`, `verify-ca` and `verify-full` fail with `TLS_ERROR` … `invalid peer certificate: Other(UnsupportedCertVersion)`, the same message as a direct `require` to 127.0.0.1:1433: the container's certificate is SQL Server's self-signed fallback, `CN=SSL_Self_Signed_Fallback`, an X.509 v1 certificate with no subject alternative name. It fails on the certificate itself, never on the name. No name could match it, so a successful `verify-full` needs a server with a real certificate.
- **Row 9** (the 8 MariaDB cases): sqlx's mysql driver takes `mariadb://`. From the fields with `prefer`, `disable` and `require`, a stored string with `ssl-mode=PREFERRED`, a pasted string, a stored string through the SSH tunnel with no port (3306) and `ssl-mode=DISABLED`, and a form `test` through the tunnel with `require` all connected to the e2e MariaDB 11.

## Settled choices

The owner decided these on 2026-10-01 (the phase 5a plan, "Decision 6, settled by the owner").

- **A. The supplied or saved password always wins.** A password the caller supplies, or the one saved under the save flags, replaces any password already in the string: in URL user info, or the `Password=`/`Pwd=` pair of a key=value string. `pg/string-with-password-saved-differs` sends the keychain's `new-pw`. A row whose flag is off and has nothing supplied still gives up, even when its string holds a password (`pg/string-with-password-not-saved`): Decision 6 doesn't touch the give-up rules.
- **B. TablePlus `tLSMode` is translated.** When a string has no `sslmode` (Postgres) or `ssl-mode` (MySQL, MariaDB), `tLSMode` is replaced, where it stands, by that parameter: 0 is prefer, 1 disable, 2 require. The rest of the string stays as it is. Cases: `pg/tableplus-url`, `add/pg-paste-tableplus`, `add/pg-paste-tableplus-tls1`, `add/pg-paste-tableplus-tls2`, `add/mysql-paste-tableplus-tls2`. Another `tLSMode` value isn't mapped (as `tablePlusTlsModeToSslMode`) and is left alone.
- **I. The wizard's SSL mode defaults to unset**, a "Default" option in the dropdown (Task 6). See 5-wizard above.

## Choices these fixtures make where the table leaves room

Each is pinned by the cases named. Change the cases if the owner decides otherwise.

- **C. A `+ssh` URL on a row with its own tunnel uses the row's tunnel** (row 7b). In `pg/tableplus-ssh-url` the URL's SSH part says port 22 and the row's tunnel says 2222. The row's (or form's) enabled tunnel wins, and the URL contributes its database part. Nothing pins a `+ssh` string on a row with no enabled tunnel. Suggested: take the URL's SSH part then (key auth when `usePrivateKey=true`, which without a key path gives `CREDENTIALS_REQUIRED`). The TablePlus-only parameters are dropped from the database part; any others are kept.
- **D. DuckDB parameters travel in a new `duckdb_config` field** (row 10). The table says to parse them out of the path, not where they go. They are passed as DuckDB config options (`access_mode=read_only` is one), so an option DuckDB doesn't know fails the connect with its own message.
- **E. An empty host is `CREDENTIALS_REQUIRED`** (row 5): phase 4's code for a missing field. It applies only where the host is used, meaning MSSQL or a string built from the fields. A stored or typed string brings its own host.
- **F. Password auth with an empty SSH password on a form is `CREDENTIALS_REQUIRED`** (rows 5 and one builder). `test/pg-ssh-password-empty`.
- **G. `tls_server_name` is set for every tunnelled MSSQL connection** (row 6), not only for the modes that verify.
- **H. A typed string ignores the form's SSL mode** (row 4). That's how autoReconnect treated a stored string.

## Changes

Record every edit here, with the rows it follows.

### 2026-10-01 (phase 5a, Task 1): the owner's settled choices

The first draft followed Decision 6's table literally where it left room. The owner then settled three choices, and the set was updated before Task 2 used it:
- **A:** `pg/string-with-password-saved-differs` sends `new-pw` (was `old-pw`); `changedBy` `["2"]` (was `["2", "4"]`).
- **B:** `pg/tableplus-url` and `add/pg-paste-tableplus` translate `tLSMode=0`; `add/pg-paste-tableplus-tls1`, `add/pg-paste-tableplus-tls2` and `add/mysql-paste-tableplus-tls2` were added (recorded in `form-add.json` first).
- **I:** the new `5-wizard` value; form inputs lost their default `disable`; `add/mariadb-fields-disable` was added as MariaDB's explicit-`disable` variant; `add/mssql-paste-url`, `add/pg-port-zero`, `add/pg-ssh-key` and `add/mysql-ssh-key` changed.

### 2026-10-01 (phase 5a, Task 2): the live checks

The 18 `needsLiveCheck` cases were checked live (see "Live checks"). Their flags were dropped and their notes now record what was seen. One output changed, as row 3 said it would if an engine rejected an empty user: `mssql/empty-username-no-string` is `CREDENTIALS_REQUIRED` (was a config with `username` `""`), and its `changedBy` is `["3"]`. 68 cases now differ from v1. The two `ConnectConfig` field names, `tls_server_name` and `duckdb_config`, are the ones Task 2 added.
