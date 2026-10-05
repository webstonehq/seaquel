/**
 * SQLite and DuckDB aren't offered on web (they
 * would open files on the server), and stay on desktop and in the demo.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";

const env = { tauri: false, web: false };

vi.mock("$lib/utils/environment", () => ({
  isTauri: () => env.tauri,
  isWeb: () => env.web,
  isDemo: () => !env.tauri && !env.web,
}));
vi.mock("$lib/services/keyring", () => ({ getKeyringService: () => ({}) }));

const {
  getFeatures,
  isDatabaseTypeAvailable,
  databaseTypeUnavailableMessage,
  assertDatabaseTypeAvailable,
} = await import("$lib/features");
const { availableDatabaseTypes, databaseTypes } =
  await import("$lib/stores/connection-wizard.svelte.js");
const { parseConnectionString } = await import("$lib/utils/connection-string");

function setEnv(which: "desktop" | "web" | "demo") {
  env.tauri = which === "desktop";
  env.web = which === "web";
}

beforeEach(() => setEnv("desktop"));

describe("sqliteSupport / duckdbSupport", () => {
  it.each([
    ["desktop", true],
    ["demo", true],
    ["web", false],
  ] as const)("on %s: %s", (which, on) => {
    setEnv(which);
    const features = getFeatures();
    expect(features.sqliteSupport).toBe(on);
    expect(features.duckdbSupport).toBe(on);
    expect(isDatabaseTypeAvailable("sqlite")).toBe(on);
    expect(isDatabaseTypeAvailable("duckdb")).toBe(on);
  });

  it("keeps the server engines on web", () => {
    setEnv("web");
    for (const type of ["postgres", "mysql", "mariadb", "mssql"] as const) {
      expect(isDatabaseTypeAvailable(type)).toBe(true);
    }
  });
});

describe("the wizard's database types", () => {
  it("offers every type on desktop", () => {
    expect(availableDatabaseTypes().map((t) => t.value)).toEqual(databaseTypes.map((t) => t.value));
  });

  it("leaves out SQLite and DuckDB on web", () => {
    setEnv("web");
    expect(availableDatabaseTypes().map((t) => t.value)).toEqual([
      "postgres",
      "mysql",
      "mariadb",
      "mssql",
    ]);
    // The full list still names them, for connections that already exist.
    expect(databaseTypes.map((t) => t.value)).toContain("sqlite");
    expect(databaseTypes.map((t) => t.value)).toContain("duckdb");
  });
});

describe("parseConnectionString", () => {
  const fileStrings = [
    "sqlite:///data/auth.db",
    "sqlite:/data/users/other/meta.db",
    "sqlite::memory:",
    "duckdb://:memory:",
    "duckdb:///tmp/x.duckdb",
  ];

  it.each(fileStrings)("parses %s on desktop", (s) => {
    expect(parseConnectionString(s).success).toBe(true);
  });

  it.each(fileStrings)("refuses %s on web with the reason", (s) => {
    setEnv("web");
    const result = parseConnectionString(s);
    expect(result.success).toBe(false);
    if (result.success) return;
    expect(result.error).toMatch(/^(SQLite|DuckDB) connections aren't available in the web app/);
  });

  it("still parses server databases on web", () => {
    setEnv("web");
    const result = parseConnectionString("postgres://u:p@db:5432/app");
    expect(result.success && result.formData.type).toBe("postgres");
  });
});

describe("assertDatabaseTypeAvailable", () => {
  it("throws the web message on web", () => {
    setEnv("web");
    expect(() => assertDatabaseTypeAvailable("sqlite")).toThrow(
      databaseTypeUnavailableMessage("sqlite"),
    );
    expect(databaseTypeUnavailableMessage("duckdb")).toContain("Use the desktop app for DuckDB");
    expect(() => assertDatabaseTypeAvailable("postgres")).not.toThrow();
  });

  it("passes on desktop", () => {
    expect(() => assertDatabaseTypeAvailable("sqlite")).not.toThrow();
    expect(() => assertDatabaseTypeAvailable("duckdb")).not.toThrow();
  });
});
