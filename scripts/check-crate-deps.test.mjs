import { describe, expect, it } from "vitest";
import { checkCrateDeps } from "./check-crate-deps.mjs";

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
    expect(checkCrateDeps([pkg("seaquel-storage")])).toEqual([
      "seaquel-storage: unclassified crate. Add it to scripts/check-crate-deps.mjs.",
    ]);
  });
});
