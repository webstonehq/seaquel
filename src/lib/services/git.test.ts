import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

const git = await import("./git");
const { CoreCallError } = await import("$lib/storage/rust-client");

/** The text of the n-th `core_call` body, checking it went out as bytes. */
function sentText(n = 0): string {
  const [command, body] = invoke.mock.calls[n] as [string, Uint8Array];
  expect(command).toBe("core_call");
  expect(body).toBeInstanceOf(Uint8Array);
  return new TextDecoder().decode(body);
}

function reply(method: string, result: unknown) {
  invoke.mockResolvedValueOnce({ method: "git", result: { method, result } });
}

// The JSON seaquel-types' snapshot test pins for GitRepoStatus and
// GitSyncResult: what the Tauri commands sent, snake_case.
const WIRE_STATUS = {
  is_clean: false,
  pending_changes: 2,
  ahead_by: 1,
  behind_by: 3,
  has_conflicts: true,
  current_branch: "main",
  modified_files: ["a.sql"],
  untracked_files: ["b.sql"],
  conflict_files: ["c.sql"],
};
const WIRE_SYNC = {
  success: false,
  message: "Merge conflicts detected",
  conflicts: ["q.sql"],
  files_changed: [],
};

describe("git service over core_call", () => {
  beforeEach(() => invoke.mockReset());

  it("sends method before params at both levels, as bytes", async () => {
    reply("resolveConflict", null);
    await git.resolveConflict("/r", "q.sql", "x\n");
    expect(sentText()).toBe(
      '{"method":"git","params":{"method":"resolveConflict","params":{"path":"/r","filePath":"q.sql","resolution":"x\\n"}}}',
    );
  });

  it("maps credentials to the snake_case wire shape and leaves them out when unset", async () => {
    reply("pull", WIRE_SYNC);
    reply("clone", null);
    reply("push", { ...WIRE_SYNC, success: true, message: "Push successful", conflicts: [] });
    await git.pullRepo("/r", { username: "alice", password: "tok", sshKeyPath: "/k" });
    await git.cloneRepo("https://h/r.git", "/r");
    await git.pushRepo("/r");
    expect(JSON.parse(sentText(0))).toEqual({
      method: "git",
      params: {
        method: "pull",
        params: {
          path: "/r",
          credentials: {
            username: "alice",
            password: "tok",
            ssh_key_path: "/k",
            ssh_passphrase: null,
          },
        },
      },
    });
    expect(sentText(1)).toBe(
      '{"method":"git","params":{"method":"clone","params":{"url":"https://h/r.git","path":"/r"}}}',
    );
    expect(sentText(2)).toBe('{"method":"git","params":{"method":"push","params":{"path":"/r"}}}');
  });

  it("maps the Rust status and sync JSON to the app's types", async () => {
    reply("status", WIRE_STATUS);
    await expect(git.getRepoStatus("/r")).resolves.toEqual({
      isClean: false,
      pendingChanges: 2,
      aheadBy: 1,
      behindBy: 3,
      hasConflicts: true,
      currentBranch: "main",
      modifiedFiles: ["a.sql"],
      untrackedFiles: ["b.sql"],
      conflictFiles: ["c.sql"],
    });
    reply("pull", WIRE_SYNC);
    await expect(git.pullRepo("/r")).resolves.toEqual({
      success: false,
      message: "Merge conflicts detected",
      conflicts: ["q.sql"],
      filesChanged: [],
    });
  });

  it("passes the other results through", async () => {
    reply("commit", "abc123");
    await expect(git.commitChanges("/r", "msg")).resolves.toBe("abc123");
    reply("remoteUrl", null);
    await expect(git.getRemoteUrl("/r")).resolves.toBeNull();
    reply("conflictContent", { base: "b", ours: "o", theirs: "t" });
    await expect(git.getConflictContent("/r", "q.sql")).resolves.toEqual({
      base: "b",
      ours: "o",
      theirs: "t",
    });
    reply("init", null);
    await expect(git.initRepo("/r")).resolves.toBeUndefined();
    reply("setRemote", null);
    await expect(git.setRemote("/r", "u")).resolves.toBeUndefined();
  });

  it("rejects with the git code, formatted like the old command errors", async () => {
    invoke.mockRejectedValueOnce({ code: "PUSH_ERROR", message: "Failed to push: rejected" });
    const error = await git.pushRepo("/r").catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CoreCallError);
    expect((error as InstanceType<typeof CoreCallError>).code).toBe("PUSH_ERROR");
    expect((error as Error).message).toBe("PUSH_ERROR: Failed to push: rejected");
  });

  it("refuses a response for another method without echoing it", async () => {
    reply("status", WIRE_STATUS);
    const error = await git.getConflictContent("/r", "q.sql").catch((e: unknown) => e);
    expect((error as InstanceType<typeof CoreCallError>).code).toBe("PROTOCOL_ERROR");
    expect((error as Error).message).not.toContain("a.sql");
  });

  it("no longer offers stageFile or discardFile", () => {
    expect("stageFile" in git).toBe(false);
    expect("discardFile" in git).toBe(false);
  });
});
