/**
 * When a connection string is kept (old rebuilt strings dropped, pasted ones
 * dropped unless they carry more than the fields), and that a stored string
 * never keeps a password.
 */
import { describe, expect, it } from "vitest";
import {
  connectionStringHasExtras,
  isLegacyBuiltString,
  legacyBuiltString,
  storedConnectionString,
  stripConnectionStringSecrets,
} from "./connection-string-rules";

const pg = {
  type: "postgres",
  host: "prod.example.com",
  port: 5432,
  databaseName: "app",
  username: "alice",
  sslMode: "disable",
};

describe("isLegacyBuiltString", () => {
  it.each([
    // As stored: password stripped, `postgres` turned into `postgresql`.
    ["postgresql://alice@prod.example.com/app?sslmode=disable", pg],
    ["postgres://alice:secret@prod.example.com/app?sslmode=disable", pg],
    [
      "postgresql://alice@prod.example.com:6543/app?sslmode=require",
      { ...pg, port: 6543, sslMode: "require" },
    ],
    [
      "mysql://root@db:3307/shop?ssl-mode=DISABLED",
      {
        type: "mysql",
        host: "db",
        port: 3307,
        databaseName: "shop",
        username: "root",
        sslMode: "disable",
      },
    ],
    [
      "mssql://sa@sql/app",
      {
        type: "mssql",
        host: "sql",
        port: 1433,
        databaseName: "app",
        username: "sa",
        sslMode: "disable",
      },
    ],
    ["sqlite:///data/a.db", { type: "sqlite", databaseName: "/data/a.db" }],
    ["duckdb://:memory:", { type: "duckdb", databaseName: "" }],
    ["postgresql://al%40ice@prod.example.com/app?sslmode=disable", { ...pg, username: "al@ice" }],
  ])("%s is the old builder's", (s, fields) => {
    expect(isLegacyBuiltString(s, fields)).toBe(true);
    expect(storedConnectionString({ ...fields, connectionString: s })).toBe("");
  });

  it.each([
    // Stale: a field was edited after the string was saved.
    ["postgresql://alice@prod.example.com/app?sslmode=disable", { ...pg, host: "staging" }],
    // Hand-typed, with something of its own.
    ["postgresql://alice@prod.example.com/app?application_name=seaquel", pg],
    ["sqlite:///data/a.db?mode=ro", { type: "sqlite", databaseName: "/data/a.db" }],
  ])("%s isn't", (s, fields) => {
    expect(isLegacyBuiltString(s, fields)).toBe(false);
  });

  it("a stale rebuilt string is kept, and so shown, rather than guessed at", () => {
    const row = { ...pg, host: "staging", connectionString: legacyBuiltString(pg) };
    expect(storedConnectionString(row)).toBe(row.connectionString);
  });
});

describe("connectionStringHasExtras", () => {
  it.each([
    "postgres://u:p@h/app",
    "postgres://u@h:5433/app?sslmode=require",
    "mysql://u@h/app?ssl-mode=REQUIRED",
    "postgresql://u@h/app?tLSMode=0&name=Prod",
    "sqlite:///a.db",
    "duckdb://:memory:",
    "",
  ])("%s says nothing the fields can't", (s) => {
    expect(connectionStringHasExtras(s)).toBe(false);
  });

  it.each([
    "postgres://u@h/app?application_name=seaquel",
    "postgresql://u@h/app?statusColor=686B6F&env=prod",
    "postgresql+ssh://deploy@bastion/alice@db/app",
    "sqlite:///a.db?mode=ro",
    "duckdb:///a.duckdb?threads=4",
    "Server=sql;Database=app;User Id=sa",
  ])("%s holds more", (s) => {
    expect(connectionStringHasExtras(s)).toBe(true);
  });
});

describe("stripConnectionStringSecrets", () => {
  it.each([
    ["postgres://alice:secret@h/app?sslmode=disable", "postgres://alice@h/app?sslmode=disable"],
    // The scheme the user typed is kept, and an untouched query isn't re-encoded.
    ["postgresql://alice@h/app?options=-c%20x", "postgresql://alice@h/app?options=-c%20x"],
    [
      "postgres://alice@h/app?password=secret&sslmode=require",
      "postgres://alice@h/app?sslmode=require",
    ],
    // TablePlus: the SSH password in the user info, the database's in the path.
    [
      "postgresql+ssh://sshu:SSHPW@bastion/dbu:DBPW@dbhost/db",
      "postgresql+ssh://sshu@bastion/dbu@dbhost/db",
    ],
    ["Server=sql;Database=app;User Id=sa;Password=pw;", "Server=sql;Database=app;User Id=sa;"],
    ["Server=sql;PWD={p;w}}d};Database=app", "Server=sql;Database=app;"],
    ['Server=sql;Password="a;""b";Database=app', "Server=sql;Database=app;"],
    ["host=db password='a b\\'c' dbname=app", "host=db dbname=app"],
    ["host=db PASSWORD=secret dbname=app", "host=db dbname=app"],
    ["sqlite:///a.db?mode=ro", "sqlite:///a.db?mode=ro"],
    // A password with `@` in it: all of it goes.
    ["postgresql+ssh://s@b/dbu:p@x@h/db", "postgresql+ssh://s@b/dbu@h/db"],
    // DuckDB options that can carry credentials go; the rest stay as typed.
    [
      "duckdb:///a.duckdb?threads=4&s3_secret_access_key=abc&S3_ACCESS_KEY_ID=k&s3_session_token=t&access_mode=READ_ONLY",
      "duckdb:///a.duckdb?threads=4&access_mode=READ_ONLY",
    ],
    ["duckdb:///a.duckdb?s3_secret_access_key=abc", "duckdb:///a.duckdb"],
  ])("%s → %s", (s, expected) => {
    expect(stripConnectionStringSecrets(s)).toBe(expected);
  });

  it.each([
    // An unescaped `#` puts part of the password in the fragment.
    "postgres://alice:12#34@h/app",
    // `new URL` can't read it.
    "postgres://alice:secret@h:notaport/app",
    "mysql://u:p?w@h/db",
    // A quote that never closes.
    "Server=sql;Password='abc",
    // A `/` in the database password of a TablePlus URL.
    "postgresql+ssh://s@b/dbu:p/w@h/db",
    // Parses as a URL, but the password is in the host.
    "sqlserver://h;password=secret",
    "sqlserver://h;PWD = secret;database=app",
  ])("%s is stored as an empty string", (s) => {
    expect(stripConnectionStringSecrets(s)).toBe("");
  });

  it("nothing stays nothing", () => {
    expect(stripConnectionStringSecrets("")).toBeUndefined();
    expect(stripConnectionStringSecrets(undefined)).toBeUndefined();
  });
});
