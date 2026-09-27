# connect-config fixtures

These files record how the TypeScript turns a saved connection and its keychain secrets into the `ConnectConfig` it sends to Core (`db_connect`), before `seaquel-workspace::connections::build_config` ports it. The TypeScript was the spec in phase 4. Since phase 5a, Task 2, the builder follows `../connect-config-v2` instead, and `tests/connect_config.rs` uses these files as the v1 side of its diff: a v2 case differs from its v1 case here exactly when its `changedBy` says so.

**The fixtures are frozen.** Phase 5 moves the GUI onto `Workspace::connect`, and after that the TS path they were recorded from is gone. Change a fixture only when the Rust behaviour is meant to differ from the TS. Say why in the change and add it to "Changes" at the end. Never regenerate one to make a failing test pass.

## How they were made

`docs/plans/artifacts/2026-09-30-freeze-connect-config.mjs.txt` ran from the repo root at `5d555e0`, with `src/lib` unmodified (phase 4, Task 2). It runs the real code, not a copy of it. esbuild bundles today's `src/lib` with types stripped and `$lib` resolved:

- **`ConnectionManager`** (`hooks/database/connection-manager.svelte.ts`) is bundled whole. `initializePersistedConnections` (lines 72-160, including the URL-username fallback), `autoReconnect`/`_autoReconnect` (790-916, secret selection), `reconnect` (380-520, password reinjection at 416-426) and `setupSshTunnel` (165-231, the host and port rewrite) all run unmodified. The class uses no runes, so it runs in Node as it is.
- **`ConnectionTabManager`** (`hooks/database/connection-tabs.svelte.ts`) is bundled whole: `open` (92-160, the prefill-to-form defaults) and `loadSavedCredentials` (195-248).
- **Also bundled unmodified:** `utils/connection-string.ts` (`getConnectionData`, `buildConnectionString`, `hasAllCredentials`), `providers/wire.ts` (`toRustConfig`), `providers/unified-tauri-provider.ts` (`connect`), `services/keyring.ts` (`TauriKeyringService`), `services/ssh-tunnel.ts` and `storage/rust-client.ts` (`callSecret`).
- **One extraction.** The reconnect tab's auto-connect lives in a Svelte component, so it's copied verbatim into a scratch module (`VIEW_EXTRACT` in the recorder). It comes from `components/connection-tab-view.svelte`:
  - lines 67-74: `formData = { ...tab.formData }`;
  - lines 79-92: syncing the loaded credentials into `formData`;
  - lines 109-122: the auto-connect gate (`hasAllCredentials`);
  - lines 124-150: `handleAutoConnect`, which runs `getConnectionData` and then `db.connections.reconnect`. Toasts, onboarding and tab removal are left out.

  Only the `$state`/`$effect` wrappers are replaced, by straight-line code in the order the effects fire. The component copies `tab.formData` before the credentials load and then syncs the non-empty ones. The extraction copies it after, which gives the same `formData`.

- **Stubbed.**
  - `@tauri-apps/api/core`'s `invoke`, which is the only I/O. `core_call` covers keychain `get`s, answered from the case's `secrets`, and SSH `open`/`close`. `open` answers `localPort = tunnelPort`. `db_connect`'s `config` argument is what gets recorded. `db_connect` succeeds, except that it refuses a config without `connection_string` for postgres, mysql and sqlite, with Core's own `CONNECTION_ERROR`, the way `seaquel-engine-{postgres,mysql,sqlite}` `driver.rs` does before contacting a server.
  - Schema loading (`$lib/engine`), which returns no tables.
  - The logger, which is captured so the recorder can read autoReconnect's "skipped" reasons.
  - Toasts, the web vault, the host-key prompt store, and `SvelteSet` (a plain `Set`).
- **Environment.** `window.__TAURI_INTERNALS__` is set, so the real `isTauri()` answers "desktop". The keychain is therefore available and unlocked, and SSH tunnels are enabled.

The output is deterministic: two runs gave byte-identical files.

## Files

One JSON array per group:

| file            | cases | covers                                                                                                                                                                                    |
| --------------- | ----- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `postgres.json` | 31    | ports, every ssl mode, strings present/missing/with a password, URL-special, `%`, unicode and `@` in user/password, empty username, username from URL, IPv6, TablePlus URLs               |
| `mysql.json`    | 11    | ports, every ssl mode (`ssl-mode` mapping), special password, empty username, passwordless                                                                                                |
| `mariadb.json`  | 4     | the `mysql` driver with `mariadb://`, rebuild, SSH                                                                                                                                        |
| `mssql.json`    | 14    | every ssl mode (`encrypt`/`trust_cert`), custom port, key=value strings (also with SSH), username from URL, unicode password, SSH                                                         |
| `sqlite.json`   | 8     | absolute, `:memory:`, spaces/unicode, Windows path, query params, no string, a stray `db:` secret, SSH on a file engine                                                                   |
| `duckdb.json`   | 8     | absolute, `:memory:`, `duckdb://`, no string, no name, no slashes, Windows path, query params                                                                                             |
| `ssh.json`      | 15    | password and key auth, passphrase saved/unsaved/missing, no key path, SSH secret unsaved/missing, no string, disabled and `null` tunnel, MySQL, special passwords, string port ≠ row port |
| `secrets.json`  | 11    | which keys are read under which flags, empty and failed reads, when the TS gives up                                                                                                       |
| `shared.json`   | 5     | rows the shape `importSharedConnections` writes (no string, empty username, flags off), and one whose flag was turned on later                                                            |

107 cases in all.

### A case

```jsonc
{
  "name": "ssh/pg-password-auth",
  "notes": "…",                 // optional
  "row": { … },                 // PersistedConnection, as connectionsRepo.loadAll returns it
  "secrets": { "db:<id>": "pw", "ssh:<id>": "ssh-pw" },   // the keychain (service app.seaquel.desktop)
  "secretErrors": ["db:<id>"],  // optional: these reads fail (a denied prompt, a locked keychain)
  "tunnelPort": 50001,          // the local port the SSH tunnel reports; null without SSH
  "load":          { "secretsRead": [...], "username": "alice", "passwordFromKeychain": true },
  "autoReconnect": { "secretsRead": [...], "connected": true, "skipped"?: "...", "tunnel"?: {...}, "config"?: {...}, "error"?: "...", "tunnelClosed"?: true },
  "reconnectTab":  { "secretsRead": [...], "hasAllCredentials": true, "autoConnected": true, "tunnel"?: {...}, "config"?: {...}, "error"?: "...", "tunnelClosed"?: true },
  "gui": "autoReconnect" | "reconnectTab" | "form"
}
```

- `row` flags are always booleans, and optional fields are absent when unset, as storage loads them. `projectId` is `default-seaquel`. The connection id is `conn-<name>`.
- `load` is app start (`initializePersistedConnections`): the keys it read, the in-memory username after the URL fallback, and whether it prefetched a password.
- `autoReconnect` is a click on a disconnected connection. `secretsRead` is only what autoReconnect itself read, after `load`.
  - `connected: false` with `skipped` means it gave up before connecting: `no password available`, `no SSH password` or `no SSH key path`.
  - `connected: false` with `error` means `reconnect` threw.
- `reconnectTab` is what happens when autoReconnect returns false. The app opens the reconnect tab with the connection as its prefill, the same object in all five callers (sidebar, connection card, command palette, connection selector, getting started). It's recorded for every case, from a fresh start, whatever autoReconnect did. `secretsRead` is what `loadSavedCredentials` read. `hasAllCredentials: false` means the form waits for the user.
- `tunnel` is the `Ssh::Open` config sent to Core. `config` is the `ConnectConfig` as it crosses IPC (JSON, so `undefined` fields are absent). `tunnelClosed` means the tunnel was closed again after a failed connect.
- `gui` says which path ends up connected in the app:
  - `autoReconnect` if it connected;
  - otherwise `reconnectTab` if the tab auto-connected without an error;
  - otherwise `form`, meaning the user has to type something.

  A real server can still refuse any of these. Only Core's offline check is modelled.

## Which path `build_config` should follow

**Follow autoReconnect. Where autoReconnect can't produce a config Core accepts, follow the reconnect tab's rebuild.** Concretely, the expected result of a case is `case[case.gui]`:

- `gui: "autoReconnect"` (71 cases): `autoReconnect.config` and `autoReconnect.tunnel`.
- `gui: "reconnectTab"` (24): `reconnectTab.config` and `reconnectTab.tunnel`. There are two kinds:
  - 23 rows with no stored string on postgres, mysql, mariadb or sqlite, where autoReconnect sends no `connection_string` and Core refuses it;
  - `mssql/key-value-string-ssh`, where the SSH rewrite can't parse the string.

  In both, the app itself only ever connects through the tab's rebuild.

- `gui: "form"` (12): `CREDENTIALS_REQUIRED` (Decision 2). These are exactly the cases where autoReconnect gives up (`skipped`).

Why:

- **autoReconnect is the app's non-interactive path**, and the one it tries first on every click. An MCP server has no one to type into a form, so it's the right model.
- **It keeps the stored string.** It preserves query parameters (`application_name`, `options`, sslrootcert paths), the string's port and IPv6 brackets.
- **Its secret rules are the ones Decision 2 states.** The tab reads SSH secrets whenever their flags are on, even without a tunnel, and even when the db password will make it give up.
- **Where both would connect with different configs, the tab's is usually the worse one.** That happens in 11 cases:
  - 6 are worse in the tab (quirks 4-7): `sslmode=disable` added to a string (`pg/string-with-password-saved-differs`), query parameters dropped (`sqlite/query-params`, `duckdb/query-params`), IPv6 broken (`pg/ipv6-host`), MSSQL encryption turned off when `sslMode` is unset (`mssql/no-sslmode`), and an SSH tunnel opened for SQLite (`sqlite/ssh-ignored`);
  - 3 differ only in the `postgres://` versus `postgresql://` scheme (`secrets/pg-save-on-missing`, `pg-save-on-empty`, `pg-read-error`);
  - 2 are better in the tab: the TablePlus URLs (`pg/tableplus-url`, `pg/tableplus-ssh-url`). autoReconnect passes TablePlus-only parameters through and mangles a `+ssh` URL. The wizard never stores such a string (it rebuilds them), so these rows can only come from an import. The expectation stays autoReconnect's.
- **The tab is otherwise needed only where autoReconnect can't work offline** (a missing string). There it's what the user's app actually does. So use it there.
- **Don't retry with the tab's config after a server-side failure.** The GUI does exactly that (autoReconnect fails, then the tab auto-connects), but for MSSQL without an `sslMode` that retry turns encryption off.

The in-memory password fallback in `_autoReconnect` (a password typed earlier in the same session) can't happen at a fresh start and isn't recorded. The MCP server has only the keychain.

## Secret selection (what the cases show)

The keychain service is `app.seaquel.desktop`, with keys `db:<id>`, `ssh:<id>` and `ssh-key:<id>`.

- **`db:<id>`**
  - It is read only when `savePassword` is on: once at load, and again in autoReconnect.
  - For SQLite and DuckDB it is read at load (the wizard defaults `savePassword` to on) but never used. autoReconnect doesn't read it and sends no password.
  - A missing secret, an empty one (`""`) and a failed read (`secretErrors`) are all the same thing: no password.
- **Giving up on the password (server engines).** With no password and `savePassword` off, autoReconnect gives up (`no password available`). That holds even when the stored string still carries a password (`pg/string-with-password-not-saved`).
- **Passwordless connections.** With `savePassword` on and no password, it connects without one (`secrets/pg-save-on-missing`, `mysql/passwordless`). The URL isn't touched and goes out verbatim, `postgresql://` included.
- **SSH secrets (autoReconnect).** These are read only when `sshTunnel.enabled`, and only after the db password check passed:
  - `ssh:<id>` when `saveSshPassword` is on;
  - `ssh-key:<id>` when `saveSshKeyPassphrase` is on.

  With key auth and `saveSshPassword` on, the SSH password is read and sent too (`secrets/all-flags-all-secrets`).

- **Giving up on SSH.** autoReconnect gives up when password auth has no SSH password (`no SSH password`: flag off, secret missing, or a failed read). It also gives up when key auth has no `keyPath` (`no SSH key path`). Key auth without a passphrase goes ahead.
- **The tab** reads `db:`, `ssh:` and `ssh-key:` by their flags alone. It ignores whether a tunnel is enabled or the engine needs a password.

## Quirks

These are recorded as the TS behaves. Task 3 kept them all except two, which it fixed in both the TS and the Rust (quirks 2 and 6; see "Changes"), and lists the ones that look like bugs in the effort log.

1. **No stored string on a sqlx engine.** Shared imports and some legacy or imported rows have no string. autoReconnect then sends `{driver}` with no `connection_string`, and Core refuses it. The app only connects through the tab, which rebuilds the string with `buildConnectionString`. After that successful reconnect the row is saved with the rebuilt string. DuckDB falls back to `databaseName` (or `:memory:`), and MSSQL ignores the string.
2. **Password reinjection re-serializes the URL.**
   - `new URL(s.replace("postgresql://", "postgres://"))`, then `url.password = pw`, gives `postgres://`.
   - Without a password the stored `postgresql://` string goes out unchanged.
   - **Fixed on 2026-09-30 (see "Changes").** As recorded, the password went to the setter raw, and the WHATWG userinfo encode set leaves `%`, `+`, `&`, `!`, `$` and `,` alone. So `50%off` became `…:50%off@…`, an invalid escape for the driver. Now it goes through `encodeURIComponent` first, which encodes all of those but `!`.
   - In the tab path the password is injected again after `buildConnectionString`'s `encodeURIComponent`. Since the fix both encode the same way.
3. **An empty username with a password** gives `postgres://:pw@host…` (`pg/empty-username`, `mysql/empty-username`).
4. **`getConnectionData` rebuilds any string that doesn't split into exactly three parts on `:`.** That covers a default port left out (which `buildConnectionString` itself does), a password in the string, IPv6, and a `:` anywhere, including query values. The rebuild:
   - drops every other query parameter;
   - adds `sslmode`/`ssl-mode` from the form;
   - drops a default port;
   - rebuilds TablePlus URLs, and `+ssh` URLs (by design);
   - drops the brackets from an IPv6 host (`postgres://alice:pw@::1/app`, broken).

   The rebuild is harmless when the string was one `buildConnectionString` wrote.

5. **The tab's prefill defaults** (`connection-tabs.svelte.ts` 105-126):
   - `sslMode` becomes `"disable"` when unset (`pg/no-sslmode` gets `sslmode=disable`; `mssql/no-sslmode` gets `encrypt: false`, where autoReconnect sends `encrypt: true, trust_cert: true`);
   - an empty host becomes `localhost`;
   - port 0 becomes 5432;
   - `save*` flags that are `undefined` become `true`;
   - the tab's tunnel config sends `password`, `keyPath` and `keyPassphrase` as `""` where autoReconnect leaves them out.
6. **MSSQL TLS mapping** (`toRustConfig`): `encrypt = sslMode !== "disable"`. The stored string is ignored entirely. **Fixed on 2026-09-30 (see "Changes"):** as recorded, `trust_cert = sslMode !== "require"`, so `verify-full` and `verify-ca` trusted any certificate. Now `trust_cert` is true only for `disable`, `allow`, `prefer` and an unset (or empty) mode; `require`, `verify-ca`, `verify-full` and anything else verify the certificate.
   - **Over an SSH tunnel those modes fail.** The config's `host` is `127.0.0.1` (the tunnel), so the certificate's name is checked against `127.0.0.1` and doesn't match. `require` already failed this way before the fix; `verify-ca` and `verify-full` now do too, where they used to connect by trusting any certificate. Recorded, not changed: the fix is a TLS server-name override (the row's host) for tunnelled MSSQL connections, a follow-up in the phase 4 plan.
7. **SSH rewrite.**
   - `setupSshTunnel` parses the stored string with `new URL` after the tunnel is open. A key=value MSSQL string throws `Invalid URL`, and the tunnel is closed (`mssql/key-value-string-ssh`).
   - On a TablePlus `postgres+ssh://` string it rewrites the outer (SSH) authority, and the db password lands on the SSH user (`pg/tableplus-ssh-url`).
   - The tunnel forwards to `row.host:row.port`, and the string's own host and port are replaced (`ssh/pg-string-port-differs`).
   - For file engines, autoReconnect ignores `sshTunnel`. The tab opens a tunnel and rewrites a SQLite string to `sqlite://127.0.0.1:<port>/Users/me/app.db` (`sqlite/ssh-ignored`).
8. **MySQL ssl-mode mapping** (`buildConnectionString`): `disable` becomes `DISABLED`, `allow`/`prefer` become `PREFERRED`, and `require` becomes `REQUIRED`. `verify-ca` and `verify-full` pass through lower-case and unmapped.
9. **MariaDB** goes to driver `mysql` with a `mariadb://` string, stored or rebuilt.
10. **File paths.**
    - DuckDB keeps query parameters in the path (`…/analytics.duckdb?access_mode=read_only`), and `duckdb://` becomes `:memory:`.
    - SQLite keeps `?mode=ro` on autoReconnect. The tab drops it (quirk 4).
    - Windows paths (`sqlite://C:\…`) split into three parts, so they are kept.
    - The tab can't auto-connect a DuckDB row with an empty `databaseName`, because `hasAllCredentials` requires one, but autoReconnect can.
11. **The URL-username fallback** percent-decodes (`j%C3%BCrgen` becomes `jürgen`). It only matters for MSSQL, which uses the fields, and for the tab's rebuild. The sqlx engines use the string as it is.

## The form path: `form-add.json` and `form-test.json` (phase 5a, Task 1)

These two files record the other way the GUI connects: a form the user filled in, sent by the connection tab's Connect button (`ConnectionManager.add`) or its Test button (`ConnectionManager.test`). They are frozen under the same rules as the files above. `tests/connect_config.rs` doesn't read them; phase 5a's replay of `connect-config-v2` does, as the v1 side of its diff.

**How they were made.** `docs/plans/artifacts/2026-10-01-freeze-connect-config.mjs.txt` is the phase 4 recorder with form cases added. It ran from the repo root at `5d12ac6` with `src/lib` unmodified. By default it writes only these two files. With `FREEZE_ALL=1` it also writes the nine phase 4 groups; run into a scratch directory, they came out byte-identical to the files here, so the bundle and stubs still model today's tree. Two runs gave byte-identical form files.

Each case runs what `connection-tab-view.svelte` does:
- **The form** starts as `defaultFormData` (`connection-tabs.svelte.ts`).
- **A pasted string** is put in `formData.connectionString`, as the paste box's `bind:value` does, and `handleParse` merges `parseConnectionString`'s fields over the form. `typed` is what the user entered afterwards on the details step.
- **Connect** calls `add({ ...getConnectionData(formData), createIfMissing })`, as `handleConnect` does.
- **Test** calls `test(getConnectionData(formData))`, as `handleTestConnection` does. It never passes `createIfMissing`.
- `add` and `test` run unmodified. Neither reinjects a password or reads the keychain; the recorder fails a case that reads it.
- `db_test` is stubbed like `db_connect`, with the same offline refusal.
- The component's `validate()` isn't run. `test/pg-empty-host` and `test/duckdb-empty-name` record forms it would stop, and their notes say so.

| file             | cases | covers                                                                                                                                                                                                                                                        |
| ---------------- | ----- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `form-add.json`  | 39    | every engine from fields; SSL modes, with an explicit `disable` for each server engine; a special password; an empty username with a password; port 0; pasted strings with query parameters, a default port, IPv6, a unicode user and TablePlus (plain with `tLSMode` 0, 1 and 2, MySQL with 2, and `+ssh`); SSH with password and key; SQLite and DuckDB `createIfMissing`, SQLite `?mode=ro`, a Windows path, SSH fields left on a SQLite form, DuckDB `?access_mode` |
| `form-test.json` | 14    | Postgres from fields and pasted; SSH with a password and with an empty one; an empty host; MySQL `verify-full`; MariaDB and MSSQL over SSH; MSSQL `verify-full` and port 0; SQLite from fields and pasted `?mode=ro`; DuckDB with no name and pasted `:memory:` |

53 cases in all.

### A form case

```jsonc
{
  "name": "add/pg-paste-params",   // add/… in form-add.json, test/… in form-test.json
  "notes": "…",                    // optional
  "op": "add" | "test",
  "paste": "postgresql://…",       // optional: the string pasted on the method step
  "typed": { "password": "pw" },   // optional: entered after the paste
  "createIfMissing": false,        // add only
  "tunnelPort": 50101,             // the local port the SSH tunnel reports; null without SSH
  "formData": { … },               // ConnectionFormData as the manager got it, secrets included
  "connectionString": "postgres://…", // what getConnectionData sent
  "tunnel": { … },                 // optional: the Ssh::Open config
  "config": { … },                 // the ConnectConfig db_connect/db_test got, as JSON
  "error": "…",                    // optional: what add/test threw (no case has one)
  "tunnelClosed": true             // optional: test closes its tunnel afterwards
}
```

### What the form path does (as recorded)

The rules are the reconnect tab's rebuild without the tab's prefill defaults and without reinjection:

- **`getConnectionData` rebuilds a typed string** in two cases: it doesn't split into exactly three parts on `:`, or it is a TablePlus URL. The rebuild:
  - turns `postgresql://` into `postgres://`;
  - drops query parameters (`add/pg-paste-params`, `add/mysql-paste-params`, `add/sqlite-paste-mode-ro`, `add/duckdb-paste-params`);
  - drops a default port;
  - adds the form's SSL mode. A paste without one keeps the wizard default `disable`, so `add/pg-paste-default-port` gets `sslmode=disable`.
- **No reinjection.** A pasted string with three parts goes out as pasted, so a password typed after it never reaches the driver (`add/pg-paste-no-password-then-typed`). `buildConnectionString` writes no user info without a username, so the password of `add/pg-empty-username-with-password` is dropped.
- **IPv6.** A pasted IPv6 host keeps its brackets through the rebuild, because the parsed host field holds them (`[::1]`). A stored row's `::1` loses them (`pg/ipv6-host`).
- **Tunnels.**
  - The request sends `password`, `keyPath` and `keyPassphrase` as `""` when the form has none.
  - `test` opens and closes a tunnel even when the SSH password is empty (`test/pg-ssh-password-empty`).
  - A SQLite form with SSH fields left on it opens a tunnel to `localhost:0` and rewrites the path into `sqlite://127.0.0.1:<port>/…` (`add/sqlite-ssh-leftover`).
- **Port 0 goes out as is:** `:0` in the Postgres string, `port: 0` for MSSQL.
- **MySQL `verify-ca`/`verify-full` pass through unmapped.** sqlx-mysql 0.8.6 refuses both (`unknown value "verify-ca" for ssl_mode`).
- **MSSQL uses the fields.** A pasted `mssql://` string without an SSL mode keeps the form's `disable`, so it connects unencrypted (`add/mssql-paste-url`).
- **`createIfMissing`** reaches only SQLite. `toRustConfig` sends DuckDB none.

## Changes

### 2026-10-01 (phase 5a, Task 1): the form-path baseline

Added `form-add.json` (39 cases) and `form-test.json` (14), recorded as described above. Four of the `form-add.json` cases (`add/pg-paste-tableplus-tls1`, `-tls2`, `add/mysql-paste-tableplus-tls2`, `add/mariadb-fields-disable`) were added the same day for the owner's settled choices B and I (see `../connect-config-v2`); re-recording left the other 49 byte-identical. No existing file changed: the nine phase 4 groups were re-recorded into a scratch directory and came out byte-identical. The intended behaviour for every case in this directory is in `../connect-config-v2`.

### 2026-09-30 (phase 4, Task 3): two fixes, in the TS and the Rust

Both were bugs in the recorded TS. They were fixed in `src/lib` (`connection-manager.svelte.ts` `reconnect`, `providers/wire.ts` `toRustConfig`) and in `seaquel-workspace`, so the app and the MCP server agree. The recorder was then re-run unchanged against the fixed tree: it changed exactly the cases below (in both their `autoReconnect` and `reconnectTab` sections where those hold the value), and a second run reproduced the updated files byte for byte.

1. **Password reinjection percent-encodes** (quirk 2): `url.password = encodeURIComponent(password)`.
   - `pg/percent-in-password`: `50%off` → `50%25off`.
   - `pg/plus-amp-equals-space-password`: `a+b&c%3Dd%20e` → `a%2Bb%26c%3Dd%20e`.
   - `pg/plus-amp-equals-space-password-rebuilt`: the same.
   - `mysql/special-password`: `…%3F%` → `…%3F%25`.
2. **MSSQL verifies the certificate for `require`, `verify-ca` and `verify-full`** (quirk 6), a security fix: `verify-full` used to trust any certificate.
   - `mssql/sslmode-verify-full`: `trust_cert` `true` → `false`.
