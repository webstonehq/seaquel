import { describe, expect, it, vi } from "vitest";

vi.mock("$lib/api/tauri", () => ({ readTablePlusConfig: vi.fn() }));
vi.mock("$lib/services/keyring", () => ({
  getKeyringService: () => ({ isAvailable: () => false }),
}));

import { mapToImportable, toTablePlusConnection } from "./tableplus-import";

/** Shape of a Connections.plist entry after the Rust side converts it to JSON. */
function plistEntry(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    ID: "abc",
    ConnectionName: "Local PG",
    Driver: "PostgreSQL",
    DatabaseHost: "127.0.0.1",
    DatabasePort: "",
    DatabaseName: "app",
    DatabaseUser: "postgres",
    DatabasePath: "",
    tLSMode: 0,
    isOverSSH: false,
    ServerAddress: "",
    ServerPort: "",
    ServerUser: "",
    isUsePrivateKey: false,
    ServerPrivateKeyName: "Import a private key...",
    ...overrides,
  };
}

function importable(overrides: Record<string, unknown> = {}, existingIds: string[] = []) {
  return mapToImportable(toTablePlusConnection(plistEntry(overrides)), existingIds);
}

describe("TablePlus import mapping", () => {
  it("maps a basic Postgres connection with default port and TLS mode", () => {
    expect(importable()).toMatchObject({
      name: "Local PG",
      type: "postgres",
      host: "127.0.0.1",
      port: 5432,
      databaseName: "app",
      username: "postgres",
      sslMode: "prefer",
      sshTunnel: undefined,
      isDuplicate: false,
      selected: true,
    });
  });

  it("maps tLSMode=1 to disable", () => {
    expect(importable({ tLSMode: 1 })?.sslMode).toBe("disable");
  });

  it("maps tLSMode=2 to require", () => {
    expect(importable({ tLSMode: 2 })?.sslMode).toBe("require");
  });

  it("leaves unknown TLS modes unmapped", () => {
    expect(importable({ tLSMode: 9 })?.sslMode).toBe(undefined);
  });

  it("carries over an SSH tunnel using password auth", () => {
    expect(
      importable({
        isOverSSH: true,
        ServerAddress: "bastion.example.com",
        ServerPort: "2222",
        ServerUser: "deploy",
      })?.sshTunnel,
    ).toEqual({
      enabled: true,
      host: "bastion.example.com",
      port: 2222,
      username: "deploy",
      authMethod: "password",
    });
  });

  it("carries over an SSH tunnel using key auth with the default port", () => {
    expect(
      importable({
        isOverSSH: true,
        ServerAddress: "bastion",
        ServerUser: "deploy",
        isUsePrivateKey: true,
      })?.sshTunnel,
    ).toMatchObject({ port: 22, authMethod: "key" });
  });

  it("ignores SSH settings when SSH is off", () => {
    expect(importable({ ServerAddress: "bastion", ServerUser: "deploy" })?.sshTunnel).toBe(
      undefined,
    );
  });

  it("uses DatabasePath for SQLite and skips SSL", () => {
    expect(
      importable({
        Driver: "SQLite",
        DatabaseName: "My notes",
        DatabasePath: "/Users/me/notes.sqlite",
      }),
    ).toMatchObject({
      type: "sqlite",
      databaseName: "/Users/me/notes.sqlite",
      sslMode: undefined,
    });
  });

  it("flags duplicates", () => {
    expect(importable({}, ["conn-127.0.0.1-5432"])).toMatchObject({
      isDuplicate: true,
      selected: false,
    });
  });

  it("skips unsupported drivers", () => {
    expect(importable({ Driver: "Redis" })).toBeNull();
  });
});
