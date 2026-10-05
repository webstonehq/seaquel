// The helper's upload into the draft (the desktop DuckDB helper plan, Task 9
// review I2), against a fake Octokit: the bytes uploaded are the bytes the
// app was pinned to, an earlier upload is replaced, and GitHub's answer is
// checked.
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { helperAsset, pinOf, RECORD_DIR, recordName } from "./release-pin.mjs";
import { uploadHelper } from "./upload-helper-asset.mjs";

const target = "aarch64-pc-windows-msvc";
const name = helperAsset(target);
const bytes = Buffer.from("the signed, gzipped helper");

/** An Octokit with only the calls the upload makes. */
function fakeGithub({ releases, answer = (args) => ({ size: args.data.length }) }) {
  const calls = [];
  return {
    calls,
    paginate: async (fn, args) => (await fn(args)).data,
    rest: {
      repos: {
        listReleases: async (args) => (calls.push(["list", args]), { data: releases }),
        getRelease: async (args) => (
          calls.push(["get", args]),
          { data: releases.find((r) => r.id === args.release_id) }
        ),
        deleteReleaseAsset: async (args) => (calls.push(["delete", args]), { data: null }),
        uploadReleaseAsset: async (args) => (calls.push(["upload", args]), { data: answer(args) }),
      },
    },
  };
}

describe("uploadHelper", () => {
  let root;
  const context = { repo: { owner: "o", repo: "r" } };
  const writeRecord = (pin) => {
    mkdirSync(join(root, RECORD_DIR), { recursive: true });
    writeFileSync(
      join(root, RECORD_DIR, recordName(target)),
      JSON.stringify({ target, asset: name, ...pin }),
    );
  };

  beforeEach(() => {
    root = mkdtempSync(join(tmpdir(), "upload-helper-"));
    mkdirSync(join(root, "src-tauri", "binaries"), { recursive: true });
    writeFileSync(join(root, "src-tauri", "binaries", name), bytes);
    writeRecord(pinOf(bytes));
  });
  afterEach(() => rmSync(root, { recursive: true, force: true }));

  it("uploads the pinned bytes into the release tauri-action reported, replacing an earlier upload", async () => {
    const github = fakeGithub({
      releases: [
        {
          id: 7,
          tag_name: "v1",
          assets: [
            { id: 70, name },
            { id: 71, name: "other" },
          ],
        },
      ],
    });
    await uploadHelper({ github, context, root, target, tag: "v1", releaseId: "7", log: () => {} });
    expect(github.calls.map((c) => c[0])).toEqual(["get", "delete", "upload"]);
    expect(github.calls[1][1].asset_id).toBe(70);
    const upload = github.calls[2][1];
    expect(upload).toMatchObject({ owner: "o", repo: "r", release_id: 7, name });
    expect(Buffer.compare(upload.data, bytes)).toBe(0);
  });

  it("finds the draft by its tag when no id is given, and wants exactly one", async () => {
    const github = fakeGithub({
      releases: [
        { id: 3, tag_name: "v1", assets: [] },
        { id: 4, tag_name: "v0" },
      ],
    });
    await uploadHelper({ github, context, root, target, tag: "v1", log: () => {} });
    expect(github.calls.at(-1)[1].release_id).toBe(3);

    const two = fakeGithub({
      releases: [
        { id: 3, tag_name: "v1" },
        { id: 5, tag_name: "v1" },
      ],
    });
    await expect(
      uploadHelper({ github: two, context, root, target, tag: "v1", log: () => {} }),
    ).rejects.toThrow("expected one release tagged v1, found 2");
  });

  it("refuses a .gz that changed after the pin, before any request", async () => {
    writeFileSync(join(root, "src-tauri", "binaries", name), Buffer.from("re-gzipped"));
    const github = fakeGithub({ releases: [{ id: 7, tag_name: "v1", assets: [] }] });
    await expect(
      uploadHelper({ github, context, root, target, tag: "v1", releaseId: "7" }),
    ).rejects.toThrow("changed after the app was pinned");
    expect(github.calls).toEqual([]);
  });

  it("checks what GitHub stored: its size, and its digest when given", async () => {
    const releases = [{ id: 7, tag_name: "v1", assets: [] }];
    const sized = fakeGithub({ releases, answer: () => ({ size: 1 }) });
    await expect(
      uploadHelper({ github: sized, context, root, target, tag: "v1", releaseId: "7" }),
    ).rejects.toThrow(`GitHub stored ${name} as 1 bytes`);
    const other = fakeGithub({
      releases,
      answer: (a) => ({ size: a.data.length, digest: `sha256:${"0".repeat(64)}` }),
    });
    await expect(
      uploadHelper({ github: other, context, root, target, tag: "v1", releaseId: "7" }),
    ).rejects.toThrow("with digest sha256:000");
    const same = fakeGithub({
      releases,
      answer: (a) => ({ size: a.data.length, digest: `sha256:${pinOf(bytes).sha256}` }),
    });
    await uploadHelper({
      github: same,
      context,
      root,
      target,
      tag: "v1",
      releaseId: "7",
      log: () => {},
    });
  });
});
