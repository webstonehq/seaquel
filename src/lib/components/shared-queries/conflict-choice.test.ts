import { describe, expect, it } from "vitest";
import { conflictChoice } from "./conflict-choice";

const content = {
  base: "base\n",
  ours: "mine\n",
  theirs: "",
  oursDeleted: false,
  theirsDeleted: true,
};

describe("conflictChoice (probe fix 5)", () => {
  it("keeping a side that deleted the file deletes it", () => {
    expect(conflictChoice(content, "theirs")).toBeNull();
    expect(
      conflictChoice({ ...content, theirsDeleted: false, oursDeleted: true }, "ours"),
    ).toBeNull();
  });

  it("keeping a side that has the file writes its text", () => {
    expect(conflictChoice(content, "ours")).toBe("mine\n");
    expect(conflictChoice(content, "base")).toBe("base\n");
    expect(conflictChoice({ ...content, theirsDeleted: false, theirs: "" }, "theirs")).toBe("");
  });
});
