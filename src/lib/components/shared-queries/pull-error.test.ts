import { describe, expect, it } from "vitest";
import { CoreCallError } from "$lib/storage/rust-client";
import { pullFailureText, refusedPaths } from "./pull-error";

const failed = (message: string) => `Pull failed: ${message}`;
const refusal = (message: string) => new CoreCallError({ code: "PULL_ERROR", message });

describe("the sync button's pull failure", () => {
  it("a fast-forward refused over uncommitted changes is said in the app's words, naming the files", () => {
    const error = refusal(
      'Commit or discard your changes to ".seaquel/projects/team/queries/q.sql", "b.sql" first',
    );
    const text = pullFailureText(error, failed);
    expect(text).toBe(
      "Nothing was pulled, so your uncommitted changes are kept. Commit or discard your changes to .seaquel/projects/team/queries/q.sql, b.sql first.",
    );
    // Neither the code nor Rust's own sentence is shown.
    expect(text).not.toContain("PULL_ERROR");
    expect(text).not.toContain('"');
  });

  it("past ten files it counts the rest", () => {
    const error = refusal('Commit or discard your changes to "a.sql", "b.sql" and 2 more first');
    expect(pullFailureText(error, failed)).toBe(
      "Nothing was pulled, so your uncommitted changes are kept. Commit or discard your changes to a.sql, b.sql and 2 more first.",
    );
  });

  it("reads paths holding a comma, a quote or the word first", () => {
    expect(
      refusedPaths(
        refusal(
          'Commit or discard your changes to "a, b.sql", "say \\"hi\\" first.sql" and 3 more first',
        ),
      ),
    ).toEqual({ paths: ["a, b.sql", 'say "hi" first.sql'], more: 3 });
  });

  it("any other failure keeps its message", () => {
    const error = new CoreCallError({ code: "PULL_ERROR", message: "Failed to fetch: timeout" });
    expect(pullFailureText(error, failed)).toBe(
      "Pull failed: PULL_ERROR: Failed to fetch: timeout",
    );
    expect(pullFailureText("offline", failed)).toBe("Pull failed: offline");
    // A refusal it can't read falls back to the message as sent.
    const garbled = refusal("Commit or discard your changes to files the pull changes first");
    expect(refusedPaths(garbled)).toBeNull();
    expect(pullFailureText(garbled, failed)).toContain("files the pull changes");
  });
});
