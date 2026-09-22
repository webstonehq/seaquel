import { describe, expect, it } from "vitest";
import { assertAllowedStorageSql } from "./storage-guard";

describe("assertAllowedStorageSql", () => {
  it.each([
    "SELECT * FROM connections",
    "  select id from projects where id = ?",
    "INSERT INTO tabs (id) VALUES (?)",
    "INSERT OR REPLACE INTO license (id) VALUES (?)",
    "REPLACE INTO license (id) VALUES (?)",
    "UPDATE tabs SET query = ? WHERE id = ?",
    "DELETE FROM tabs WHERE id = ?",
    "WITH x AS (SELECT 1) SELECT * FROM x",
    "-- comment\nSELECT 1",
    "/* block */ SELECT 1",
    "(SELECT 1)",
  ])("allows %s", (sql) => {
    expect(() => assertAllowedStorageSql(sql)).not.toThrow();
  });

  it.each([
    "ATTACH DATABASE '/data/auth.db' AS a",
    "attach '/data/auth.db' as a",
    "DETACH a",
    "VACUUM INTO '/tmp/x.db'",
    "PRAGMA writable_schema = ON",
    "-- hi\nATTACH '/data/auth.db' AS a",
    "/* SELECT */ ATTACH '/data/auth.db' AS a",
    "/* unterminated",
    "DROP TABLE connections",
    "CREATE TABLE t (id)",
    "ALTER TABLE t ADD COLUMN c",
    "BEGIN",
    "",
    "   ",
  ])("rejects %s", (sql) => {
    expect(() => assertAllowedStorageSql(sql)).toThrow();
  });

  it("rejects non-strings", () => {
    expect(() => assertAllowedStorageSql(undefined as unknown as string)).toThrow();
    expect(() => assertAllowedStorageSql(42 as unknown as string)).toThrow();
  });
});
