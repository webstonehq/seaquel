/**
 * `errorText`: how a failed call reads in the grid and toasts.
 */

import { describe, expect, it } from "vitest";
import { m } from "$lib/paraglide/messages.js";
import { errorText } from "./error-text";

describe("errorText", () => {
  it("shows the web server's per-user cap on large calls as a sentence (probe M4)", () => {
    const text = errorText("TOO_MANY_REQUESTS", "At most 4 large requests can run at once.");
    expect(text).toBe(m.rpc_too_many_requests());
    expect(text).not.toContain("TOO_MANY_REQUESTS");
  });

  it("shows a refused body's limit with its code (probe I2)", () => {
    expect(errorText("INVALID_ARGUMENT", "One request can send at most 20 MiB.")).toBe(
      "INVALID_ARGUMENT: One request can send at most 20 MiB.",
    );
  });

  it("words the DuckDB helper's engine codes (desktop DuckDB helper plan, Decision 10)", () => {
    expect(errorText("ENGINE_UNAVAILABLE", "silent")).toBe(m.duckdb_helper_unavailable());
    expect(errorText("ENGINE_NOT_AVAILABLE", "no duckdb")).toBe(m.duckdb_helper_not_available());
    // Core's own sentence says which version and why.
    expect(
      errorText("ENGINE_NOT_INSTALLED", "DuckDB support for Seaquel 2026.10.1 isn't installed: x"),
    ).toBe("DuckDB support for Seaquel 2026.10.1 isn't installed: x");
  });
});
