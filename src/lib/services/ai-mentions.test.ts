/**
 * The `@` popover offers nothing when the chat's connection doesn't share
 * its schema (phase 6, Decision 5; Core resolves a typed mention to the
 * bare name then).
 */
import { describe, expect, it } from "vitest";
import type { Dashboard, SavedQuery, SchemaTable } from "$lib/types";
import { mentionItemsFor } from "./ai-mentions";

const tables = [{ schema: "public", name: "users", columns: [] }] as unknown as SchemaTable[];
const queries = [{ name: "Monthly revenue" }] as unknown as SavedQuery[];
const dashboards = [{ name: "Sales" }] as unknown as Dashboard[];

describe("the mention popover", () => {
  it("offers tables, saved queries and dashboards with schema sharing on", () => {
    expect(mentionItemsFor(true, tables, queries, dashboards).map((i) => i.token)).toEqual([
      "public.users",
      "Monthly revenue",
      "Sales",
    ]);
  });

  it("offers nothing with schema sharing off", () => {
    expect(mentionItemsFor(false, tables, queries, dashboards)).toEqual([]);
  });
});
