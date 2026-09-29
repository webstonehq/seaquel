import { describe, expect, it, vi } from "vitest";

vi.mock("$lib/api/tauri", () => ({ readDbeaverConfig: vi.fn() }));

import { mapToImportable } from "./dbeaver-import";

type Existing = Parameters<typeof mapToImportable>[1];

function importable(configuration: Record<string, string> = {}, existing: Existing = []) {
  return mapToImportable(
    {
      id: "postgres-jdbc-1",
      provider: "postgresql",
      driver: "postgres-jdbc",
      name: "Local PG",
      configuration: { host: "db", port: "5432", database: "app", user: "me", ...configuration },
    },
    existing,
  );
}

const savedPg = {
  type: "postgres",
  host: "db",
  port: 5432,
  databaseName: "app",
  username: "me",
} as Existing[number];

describe("DBeaver import mapping", () => {
  it("maps a Postgres connection", () => {
    expect(importable()).toMatchObject({
      type: "postgres",
      host: "db",
      port: 5432,
      databaseName: "app",
      username: "me",
      isDuplicate: false,
      selected: true,
    });
  });

  it("flags duplicates: same type, host, port, database and user", () => {
    expect(importable({}, [savedPg])).toMatchObject({ isDuplicate: true, selected: false });
  });

  it("two connections on one host and port aren't duplicates of each other", () => {
    expect(importable({ database: "other" }, [savedPg])?.isDuplicate).toBe(false);
    expect(importable({ user: "reader" }, [savedPg])?.isDuplicate).toBe(false);
  });
});
