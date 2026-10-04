import { execFileSync } from "node:child_process";
import { describe, expect, it } from "vitest";
import { checkCrateDeps } from "./check-crate-deps.mjs";

const INFRA = [
  "seaquel-storage",
  "seaquel-secrets",
  "seaquel-ssh",
  "seaquel-git",
  "seaquel-license",
  "seaquel-http",
  "seaquel-ai",
];

/** A `cargo metadata` package; string deps are normal dependencies. */
const pkg = (name, ...deps) => ({
  name,
  dependencies: deps.map((d) => (typeof d === "string" ? { name: d, kind: null } : d)),
});

describe("checkCrateDeps", () => {
  it("accepts the phase 0 workspace", () => {
    const packages = [
      pkg("seaquel-macros"),
      pkg("seaquel-runtime", "seaquel-macros"),
      pkg("seaquel-types"),
      pkg("seaquel-engine", "seaquel-runtime", "seaquel-types"),
      pkg("seaquel-engine-testkit", "seaquel-engine"),
      pkg("seaquel-engine-sqlite", "seaquel-engine", "seaquel-runtime", {
        name: "seaquel-engine-testkit",
        kind: "dev",
      }),
      pkg("seaquel-core", "seaquel-engine", "seaquel-types", "seaquel-engine-sqlite"),
      pkg("seaquel-rpc", "seaquel-core", "seaquel-engine", "seaquel-types"),
      pkg("seaquel-server", "seaquel-core", "seaquel-types", "seaquel-rpc"),
      pkg("seaquel", "seaquel-core", "seaquel-types", "seaquel-rpc"),
    ];
    expect(checkCrateDeps(packages)).toEqual([]);
  });

  it("accepts the phase 2b workspace", () => {
    const packages = [
      pkg("seaquel-macros"),
      pkg("seaquel-runtime", "seaquel-macros"),
      pkg("seaquel-types"),
      pkg("seaquel-engine", "seaquel-runtime", "seaquel-types"),
      pkg("seaquel-sql", "seaquel-types"),
      pkg("seaquel-wasm", "seaquel-sql", "seaquel-types"),
      pkg("seaquel-engine-postgres", "seaquel-engine", "seaquel-runtime", "seaquel-sql"),
      pkg("seaquel-core", "seaquel-engine", "seaquel-types", "seaquel-engine-postgres"),
    ];
    expect(checkCrateDeps(packages)).toEqual([]);
  });

  it("rejects the wasm glue reaching Core", () => {
    const errors = checkCrateDeps([
      pkg("seaquel-wasm", "seaquel-sql", "seaquel-core"),
      pkg("seaquel-sql"),
      pkg("seaquel-core"),
    ]);
    expect(errors).toEqual([
      "seaquel-wasm -> seaquel-core: wasm glue may only depend on pure crates",
    ]);
  });

  it("rejects an engine depending on the wasm glue", () => {
    const errors = checkCrateDeps([
      pkg("seaquel-engine-postgres", "seaquel-engine", "seaquel-wasm"),
      pkg("seaquel-engine"),
      pkg("seaquel-wasm"),
    ]);
    expect(errors).toEqual([
      "seaquel-engine-postgres -> seaquel-wasm: engine crates may only depend on seaquel-engine, seaquel-runtime, seaquel-types and seaquel-sql",
    ]);
  });

  it("rejects seaquel-sql depending on an engine", () => {
    const errors = checkCrateDeps([
      pkg("seaquel-sql", "seaquel-engine-postgres"),
      pkg("seaquel-engine-postgres"),
    ]);
    expect(errors).toEqual([
      "seaquel-sql -> seaquel-engine-postgres: pure crates may only depend on other pure crates",
    ]);
  });

  it("rejects an engine depending on another engine", () => {
    const errors = checkCrateDeps([
      pkg("seaquel-engine-postgres", "seaquel-engine-mysql"),
      pkg("seaquel-engine-mysql"),
    ]);
    expect(errors).toHaveLength(1);
    expect(errors[0]).toMatch(/^seaquel-engine-postgres -> seaquel-engine-mysql/);
  });

  it("rejects a pure crate depending on a native one", () => {
    const errors = checkCrateDeps([
      pkg("seaquel-engine", "seaquel-engine-sqlite"),
      pkg("seaquel-engine-sqlite"),
    ]);
    expect(errors[0]).toMatch(/pure crates may only depend on other pure crates/);
  });

  it("rejects an interface bypassing Core", () => {
    const errors = checkCrateDeps([
      pkg("seaquel-server", "seaquel-engine-postgres"),
      pkg("seaquel-engine-postgres"),
    ]);
    expect(errors[0]).toMatch(/interfaces reach everything through seaquel-core/);
  });

  it("rejects interface glue naming an engine", () => {
    const errors = checkCrateDeps([
      pkg("seaquel-rpc", "seaquel-core", "seaquel-engine-postgres"),
      pkg("seaquel-core"),
      pkg("seaquel-engine-postgres"),
    ]);
    expect(errors).toEqual([
      "seaquel-rpc -> seaquel-engine-postgres: interface glue may only depend on seaquel-core, seaquel-engine, seaquel-runtime and seaquel-types",
    ]);
  });

  it("rejects the testkit naming an engine", () => {
    const errors = checkCrateDeps([
      pkg("seaquel-engine-testkit", "seaquel-engine-sqlite"),
      pkg("seaquel-engine-sqlite"),
    ]);
    expect(errors[0]).toMatch(/EngineRegistry/);
  });

  it("ignores dev-dependencies and third-party crates", () => {
    const errors = checkCrateDeps([
      pkg("seaquel-engine-sqlite", "sqlx", { name: "seaquel-engine-testkit", kind: "dev" }),
      pkg("seaquel-engine-testkit"),
    ]);
    expect(errors).toEqual([]);
  });

  it("requires every crate to be classified", () => {
    expect(checkCrateDeps([pkg("seaquel-unclassified")])).toEqual([
      "seaquel-unclassified: unclassified crate. Add it to scripts/check-crate-deps.mjs.",
    ]);
  });

  it("accepts the phase 3 infrastructure crates behind Core", () => {
    const packages = [
      pkg("seaquel-runtime"),
      pkg("seaquel-types"),
      ...INFRA.map((name) => pkg(name, "seaquel-runtime", "seaquel-types")),
      pkg("seaquel-core", "seaquel-runtime", ...INFRA),
      pkg("seaquel-server", "seaquel-core", "seaquel-runtime"),
      pkg("seaquel", "seaquel-core", "seaquel-runtime"),
    ];
    expect(checkCrateDeps(packages)).toEqual([]);
  });

  it("rejects an interface naming an infrastructure crate", () => {
    const errors = checkCrateDeps([
      pkg("seaquel-server", "seaquel-core", "seaquel-license"),
      pkg("seaquel", "seaquel-core", "seaquel-secrets"),
      pkg("seaquel-core"),
      pkg("seaquel-license"),
      pkg("seaquel-secrets"),
    ]);
    expect(errors).toEqual([
      "seaquel-server -> seaquel-license: interfaces reach infrastructure crates through seaquel-core (e.g. core.license_server())",
      "seaquel -> seaquel-secrets: interfaces reach infrastructure crates through seaquel-core (e.g. core.license_server())",
    ]);
  });

  it("rejects an infrastructure crate naming an engine", () => {
    const errors = checkCrateDeps([
      pkg("seaquel-storage", "seaquel-engine-sqlite"),
      pkg("seaquel-engine-sqlite"),
    ]);
    expect(errors).toEqual([
      "seaquel-storage -> seaquel-engine-sqlite: reach engines through EngineRegistry, never by crate name",
    ]);
  });

  it("accepts the phase 4 crates: the workspace domain behind Core, the MCP and CLI interfaces", () => {
    const packages = [
      pkg("seaquel-runtime"),
      pkg("seaquel-types"),
      pkg("seaquel-sql"),
      pkg("seaquel-storage", "seaquel-types"),
      pkg("seaquel-secrets"),
      pkg(
        "seaquel-workspace",
        "seaquel-types",
        "seaquel-sql",
        "seaquel-storage",
        "seaquel-secrets",
      ),
      pkg(
        "seaquel-core",
        "seaquel-types",
        "seaquel-workspace",
        "seaquel-storage",
        "seaquel-secrets",
      ),
      pkg("seaquel-rpc", "seaquel-core", "seaquel-types"),
      pkg("seaquel-mcp", "seaquel-core", "seaquel-rpc", "seaquel-types"),
      pkg("seaquel-cli", "seaquel-core", "seaquel-mcp", "seaquel-rpc", "seaquel-types"),
    ];
    expect(checkCrateDeps(packages)).toEqual([]);
  });

  it("rejects seaquel-cli depending on seaquel-workspace directly", () => {
    const errors = checkCrateDeps([
      pkg("seaquel-cli", "seaquel-core", "seaquel-workspace"),
      pkg("seaquel-core"),
      pkg("seaquel-workspace"),
    ]);
    expect(errors).toEqual([
      "seaquel-cli -> seaquel-workspace: interfaces reach infrastructure crates through seaquel-core (e.g. core.license_server())",
    ]);
  });

  it("rejects an interface depending on an interface other than seaquel-mcp", () => {
    const errors = checkCrateDeps([
      pkg("seaquel-mcp", "seaquel-core", "seaquel-cli"),
      pkg("seaquel-server", "seaquel-core", "seaquel-mcp"),
      pkg("seaquel-core"),
      pkg("seaquel-cli", "seaquel-core"),
    ]);
    expect(errors).toEqual([
      "seaquel-mcp -> seaquel-cli: interfaces reach everything through seaquel-core",
    ]);
  });

  it("accepts the phase 7a terminal crates", () => {
    const packages = [
      pkg("seaquel-core"),
      pkg("seaquel-runtime"),
      pkg("seaquel-types"),
      pkg("seaquel-rpc", "seaquel-core", "seaquel-types"),
      pkg("seaquel-mcp", "seaquel-core", "seaquel-rpc", "seaquel-types"),
      pkg("seaquel-terminal", "seaquel-core", "seaquel-runtime", "seaquel-types"),
      pkg("seaquel-cli", "seaquel-core", "seaquel-mcp", "seaquel-terminal"),
      pkg("seaquel-tui", "seaquel-core", "seaquel-terminal", "seaquel-runtime", "seaquel-types"),
    ];
    expect(checkCrateDeps(packages)).toEqual([]);
  });

  it("keeps the MCP server out of the TUI and seaquel-terminal to Core", () => {
    const base = [
      pkg("seaquel-core"),
      pkg("seaquel-rpc", "seaquel-core"),
      pkg("seaquel-mcp", "seaquel-core"),
      pkg("seaquel-storage"),
    ];
    expect(
      checkCrateDeps([
        ...base,
        pkg("seaquel-terminal", "seaquel-core"),
        pkg("seaquel-tui", "seaquel-core", "seaquel-terminal", "seaquel-mcp", "seaquel-storage"),
      ]),
    ).toEqual([
      "seaquel-tui -> seaquel-mcp: seaquel-tui doesn't link the MCP server (rmcp); only seaquel-cli does",
      "seaquel-tui -> seaquel-storage: interfaces reach infrastructure crates through seaquel-core (e.g. core.license_server())",
    ]);
    expect(
      checkCrateDeps([
        ...base,
        pkg("seaquel-terminal", "seaquel-core", "seaquel-mcp", "seaquel-rpc"),
      ]),
    ).toEqual([
      "seaquel-terminal -> seaquel-mcp: seaquel-terminal may use only seaquel-core, seaquel-runtime and seaquel-types",
      "seaquel-terminal -> seaquel-rpc: seaquel-terminal may use only seaquel-core, seaquel-runtime and seaquel-types",
    ]);
    // Nothing depends on the TUI.
    expect(
      checkCrateDeps([
        ...base,
        pkg("seaquel-tui", "seaquel-core"),
        pkg("seaquel-cli", "seaquel-tui"),
      ]),
    ).toEqual(["seaquel-cli -> seaquel-tui: interfaces reach everything through seaquel-core"]);
  });

  it("rejects seaquel-mcp bypassing Core for an engine", () => {
    const errors = checkCrateDeps([
      pkg("seaquel-mcp", "seaquel-core", "seaquel-engine-postgres"),
      pkg("seaquel-core"),
      pkg("seaquel-engine-postgres"),
    ]);
    expect(errors).toEqual([
      "seaquel-mcp -> seaquel-engine-postgres: interfaces reach everything through seaquel-core",
    ]);
  });

  it("rejects seaquel-workspace naming an engine crate", () => {
    const errors = checkCrateDeps([
      pkg("seaquel-workspace", "seaquel-types", "seaquel-engine-mssql"),
      pkg("seaquel-types"),
      pkg("seaquel-engine-mssql"),
    ]);
    expect(errors).toEqual([
      "seaquel-workspace -> seaquel-engine-mssql: reach engines through EngineRegistry, never by crate name",
    ]);
  });

  it("lets seaquel-browser register DuckDB's browser driver, and nothing else of the kind", () => {
    const base = [
      pkg("seaquel-core"),
      pkg("seaquel-rpc", "seaquel-core"),
      pkg("seaquel-runtime"),
      pkg("seaquel-types"),
      pkg("seaquel-engine-duckdb"),
      pkg("seaquel-engine-sqlite"),
      pkg("seaquel-storage"),
    ];
    expect(
      checkCrateDeps([
        ...base,
        pkg(
          "seaquel-browser",
          "seaquel-core",
          "seaquel-rpc",
          "seaquel-runtime",
          "seaquel-types",
          "seaquel-engine-duckdb",
        ),
      ]),
    ).toEqual([]);
    expect(
      checkCrateDeps([
        ...base,
        pkg("seaquel-browser", "seaquel-core", "seaquel-engine-sqlite", "seaquel-storage"),
      ]),
    ).toEqual([
      "seaquel-browser -> seaquel-engine-sqlite: the browser module may name only seaquel-engine-duckdb's browser driver; other engines go through seaquel-core",
      "seaquel-browser -> seaquel-storage: interfaces reach infrastructure crates through seaquel-core (e.g. core.license_server())",
    ]);
    // Other interfaces still may not name the DuckDB crate.
    expect(
      checkCrateDeps([...base, pkg("seaquel-server", "seaquel-core", "seaquel-engine-duckdb")]),
    ).toEqual([
      "seaquel-server -> seaquel-engine-duckdb: interfaces reach everything through seaquel-core",
    ]);
  });

  it("accepts the phase 6 crates", () => {
    const packages = [
      pkg("seaquel-macros"),
      pkg("seaquel-runtime", "seaquel-macros"),
      pkg("seaquel-types"),
      pkg("seaquel-sql", "seaquel-types"),
      pkg("seaquel-workspace", "seaquel-types", "seaquel-sql"),
      pkg("seaquel-ai", "seaquel-runtime", "seaquel-types", "seaquel-sql", "seaquel-workspace"),
      pkg("seaquel-http", "seaquel-ai", "seaquel-runtime"),
      pkg("seaquel-license", "seaquel-http", "seaquel-types"),
      pkg("seaquel-core", "seaquel-ai", "seaquel-http", "seaquel-license"),
    ];
    expect(checkCrateDeps(packages)).toEqual([]);
  });

  it("keeps seaquel-ai wasm-clean and behind Core", () => {
    const base = [pkg("seaquel-runtime"), pkg("seaquel-http"), pkg("seaquel-storage")];
    expect(
      checkCrateDeps([
        ...base,
        pkg("seaquel-ai", "seaquel-runtime", "seaquel-http", "seaquel-storage"),
      ]),
    ).toEqual([
      "seaquel-ai -> seaquel-http: seaquel-ai builds for wasm32; it may only depend on pure crates and seaquel-workspace",
      "seaquel-ai -> seaquel-storage: seaquel-ai builds for wasm32; it may only depend on pure crates and seaquel-workspace",
    ]);
    expect(
      checkCrateDeps([
        ...base,
        pkg("seaquel-ai"),
        pkg("seaquel-server", "seaquel-ai", "seaquel-http"),
      ]),
    ).toEqual([
      "seaquel-server -> seaquel-ai: interfaces reach infrastructure crates through seaquel-core (e.g. core.license_server())",
      "seaquel-server -> seaquel-http: interfaces reach infrastructure crates through seaquel-core (e.g. core.license_server())",
    ]);
  });

  it("rejects seaquel-server depending on seaquel-license in this workspace's metadata", () => {
    const metadata = execFileSync("cargo", ["metadata", "--format-version", "1", "--no-deps"], {
      encoding: "utf8",
      maxBuffer: 64 * 1024 * 1024,
    });
    const { packages } = JSON.parse(metadata);
    expect(checkCrateDeps(packages)).toEqual([]);

    const tampered = structuredClone(packages);
    const server = tampered.find((p) => p.name === "seaquel-server");
    server.dependencies.push({ name: "seaquel-license", kind: null });
    expect(checkCrateDeps(tampered)).toEqual([
      "seaquel-server -> seaquel-license: interfaces reach infrastructure crates through seaquel-core (e.g. core.license_server())",
    ]);
  }, 60_000);
});
