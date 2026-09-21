import { describe, expect, it, vi } from "vitest";

vi.mock("$lib/services/keyring", () => ({
  getKeyringService: () => ({ isAvailable: () => false }),
}));

import { getConnectionData, parseConnectionString } from "./connection-string";

function parse(connStr: string) {
  const result = parseConnectionString(connStr);
  if (!result.success) throw new Error(`Expected success, got: ${result.error}`);
  return result.formData;
}

describe("parseConnectionString", () => {
  it("parses a basic postgres URL", () => {
    expect(parse("postgres://alice:secret@db.example.com:6543/app")).toMatchObject({
      type: "postgres",
      host: "db.example.com",
      port: 6543,
      databaseName: "app",
      username: "alice",
      password: "secret",
      name: "app",
      sshEnabled: false,
    });
  });

  it("decodes the database name", () => {
    expect(parse("postgres://u@h/my%20db").databaseName).toBe("my db");
  });

  describe("TablePlus URLs", () => {
    it("uses name and tLSMode, ignoring TablePlus-only params", () => {
      const formData = parse(
        "postgresql://alice:secret@db.example.com:5432/app?statusColor=686B6F&env=local&name=My%20DB&tLSMode=0&usePrivateKey=false&safeModeLevel=0&advancedSafeModeLevel=0&driverVersion=0&lazyload=false",
      );
      expect(formData).toMatchObject({
        type: "postgres",
        host: "db.example.com",
        port: 5432,
        databaseName: "app",
        username: "alice",
        password: "secret",
        name: "My DB",
        sslMode: "prefer",
        sshEnabled: false,
      });
    });

    it.each([
      ["0", "prefer"],
      ["1", "disable"],
      ["2", "require"],
    ])("maps tLSMode=%s to sslMode %s", (tlsMode, sslMode) => {
      expect(parse(`postgresql://u@h/db?tLSMode=${tlsMode}`).sslMode).toBe(sslMode);
    });

    it("leaves unknown tLSMode values unmapped", () => {
      expect(parse("postgresql://u@h/db?tLSMode=9")).not.toHaveProperty("sslMode");
    });

    it("prefers an explicit sslmode over tLSMode", () => {
      expect(parse("postgresql://u@h/db?sslmode=require&tLSMode=0").sslMode).toBe("require");
    });

    it("parses an SSH URL with passwords", () => {
      const formData = parse(
        "postgresql+ssh://deploy:sshpass@bastion.example.com:2222/dbuser:dbpass@internal-pg:5433/mydb?name=Prod&usePrivateKey=false",
      );
      expect(formData).toMatchObject({
        type: "postgres",
        host: "internal-pg",
        port: 5433,
        databaseName: "mydb",
        username: "dbuser",
        password: "dbpass",
        name: "Prod",
        sshEnabled: true,
        sshHost: "bastion.example.com",
        sshPort: 2222,
        sshUsername: "deploy",
        sshAuthMethod: "password",
        sshPassword: "sshpass",
      });
    });

    it("parses an SSH URL using a private key with default ports", () => {
      const formData = parse(
        "mysql+ssh://ec2-user@jump.example.com/root@10.0.0.5/shop?usePrivateKey=true",
      );
      expect(formData).toMatchObject({
        type: "mysql",
        host: "10.0.0.5",
        port: 3306,
        databaseName: "shop",
        username: "root",
        password: "",
        sshEnabled: true,
        sshHost: "jump.example.com",
        sshPort: 22,
        sshUsername: "ec2-user",
        sshAuthMethod: "key",
        sshPassword: "",
      });
    });

    it("defaults the database host to 127.0.0.1 in SSH URLs", () => {
      const formData = parse("postgresql+ssh://ubuntu@bastion/dbuser@:5432/mydb");
      expect(formData).toMatchObject({ host: "127.0.0.1", port: 5432, databaseName: "mydb" });
    });

    it("decodes encoded credentials in SSH URLs", () => {
      const formData = parse("postgresql+ssh://u%40x:p%2Fw@bastion/db%40user:p%40ss@db/app");
      expect(formData).toMatchObject({
        sshUsername: "u@x",
        sshPassword: "p/w",
        username: "db@user",
        password: "p@ss",
      });
    });

    it("strips query params from SQLite paths", () => {
      expect(parse("sqlite:///Users/me/data.sqlite?statusColor=686B6F&name=Local")).toMatchObject({
        type: "sqlite",
        databaseName: "/Users/me/data.sqlite",
        name: "Local",
      });
    });
  });

  it("rejects unsupported schemes", () => {
    expect(parseConnectionString("redis+ssh://u@h/x@y/0")).toMatchObject({ success: false });
    expect(parseConnectionString("mongodb://u@h/db")).toMatchObject({ success: false });
  });
});

describe("getConnectionData", () => {
  const baseForm = {
    name: "x",
    type: "postgres" as const,
    host: "db.example.com",
    port: 5432,
    databaseName: "app",
    username: "alice",
    password: "",
    sslMode: "require",
    sshEnabled: false,
    sshHost: "",
    sshPort: 22,
    sshUsername: "",
    sshAuthMethod: "password" as const,
    sshPassword: "",
    sshKeyPath: "",
    sshKeyPassphrase: "",
    savePassword: true,
    saveSshPassword: true,
    saveSshKeyPassphrase: true,
  };

  it("passes a plain connection string through", () => {
    const connectionString = "postgres://alice@db.example.com:5432/app?options=-c%20foo";
    expect(getConnectionData({ ...baseForm, connectionString }).connectionString).toBe(
      connectionString,
    );
  });

  it.each([
    "postgresql://alice@db.example.com:5432/app?statusColor=686B6F&name=App",
    "postgresql+ssh://deploy@bastion:22/alice@db.example.com/app",
  ])("rebuilds TablePlus URLs from form fields: %s", (connectionString) => {
    expect(getConnectionData({ ...baseForm, connectionString }).connectionString).toBe(
      "postgres://alice@db.example.com/app?sslmode=require",
    );
  });
});
