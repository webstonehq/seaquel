import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { RUST_ENV_NAMES, RUST_HOME, rustEnv } from "./rust-env.js";

describe("rustEnv", () => {
  const node = {
    PATH: "/usr/bin",
    DATA_DIR: "/data",
    TZ: "UTC",
    https_proxy: "http://proxy:3128",
    NODE_EXTRA_CA_CERTS: "/etc/ca.pem",
    SEAQUEL_CONTROL_URL: "https://control",
    // Must not reach Rust.
    PGPASSWORD: "hunter2",
    PGUSER: "admin",
    PGHOST: "prod-db",
    PGPASSFILE: "/root/.pgpass",
    PGSSLKEY: "/secrets/client.key",
    MYSQL_PWD: "hunter2",
    MYSQL_HOST: "prod-mysql",
    SEAQUEL_AUTH_SECRET: "session-signing-key",
    AWS_SECRET_ACCESS_KEY: "aws",
    DATABASE_URL: "postgres://admin:hunter2@prod-db/app",
    HOME: "/home/node",
    NODE_OPTIONS: "--max-old-space-size=8192",
  };

  it("passes only allow-listed names, points HOME nowhere, and applies overrides", () => {
    expect(rustEnv(node, { BIND_ADDR: "127.0.0.1:8788", SEAQUEL_INTERNAL_SECRET: "s" })).toEqual({
      PATH: "/usr/bin",
      DATA_DIR: "/data",
      TZ: "UTC",
      https_proxy: "http://proxy:3128",
      NODE_EXTRA_CA_CERTS: "/etc/ca.pem",
      SEAQUEL_CONTROL_URL: "https://control",
      HOME: RUST_HOME,
      BIND_ADDR: "127.0.0.1:8788",
      SEAQUEL_INTERNAL_SECRET: "s",
    });
  });

  it("drops every libpq and MySQL client variable", () => {
    const env = rustEnv(Object.fromEntries(Object.keys(node).map((k) => [k, "x"])));
    for (const name of Object.keys(env)) {
      expect(name).not.toMatch(/^(PG|MYSQL)/);
    }
  });

  it("skips undefined values", () => {
    expect(rustEnv({ PATH: undefined, TZ: "UTC" })).toEqual({ TZ: "UTC", HOME: RUST_HOME });
  });
});

/** Every `.rs` file under `dir`. */
function rustFiles(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return rustFiles(path);
    return path.endsWith(".rs") ? [path] : [];
  });
}

describe("the allow-list covers what the Rust service reads", () => {
  it("names every env var seaquel-server and seaquel-license read", () => {
    const names = new Set<string>();
    for (const dir of ["crates/seaquel-server/src", "crates/seaquel-license/src"]) {
      for (const file of rustFiles(dir)) {
        const src = readFileSync(file, "utf8");
        for (const m of src.matchAll(/_ENV: &str = "([A-Z0-9_]+)"/g)) names.add(m[1]);
        for (const m of src.matchAll(/env::var(?:_os)?\("([A-Z0-9_]+)"\)/g)) names.add(m[1]);
      }
    }
    expect(names.size).toBeGreaterThan(5);
    for (const name of names) {
      expect(RUST_ENV_NAMES.has(name), name).toBe(true);
    }
  });
});
