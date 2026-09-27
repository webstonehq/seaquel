/**
 * `toRustConfig`'s MSSQL TLS mapping. A mode that asks for verification
 * (`require`, `verify-ca`, `verify-full`) must never trust any certificate;
 * `seaquel-workspace`'s `mssql_trusts_any_cert` follows the same rule.
 */
import { describe, expect, it } from "vitest";
import { toRustConfig } from "./wire";

function mssql(sslMode: string | undefined) {
  return toRustConfig({
    type: "mssql",
    host: "sql.example.com",
    port: 1433,
    databaseName: "sales",
    username: "sa",
    password: "pw",
    sslMode,
  });
}

describe("toRustConfig (mssql)", () => {
  it.each([
    [undefined, true, true],
    ["", true, true],
    ["disable", false, true],
    ["allow", true, true],
    ["prefer", true, true],
    ["require", true, false],
    ["verify-ca", true, false],
    ["verify-full", true, false],
  ])("sslMode %s gives encrypt %s and trust_cert %s", (mode, encrypt, trust) => {
    const config = mssql(mode);
    expect(config.encrypt).toBe(encrypt);
    expect(config.trust_cert).toBe(trust);
  });
});
