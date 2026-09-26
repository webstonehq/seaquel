import { beforeEach, describe, expect, it, vi } from "vitest";
import type { RepoStatus, SharedQueryRepo } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";

vi.mock("$lib/services/git", () => ({
  getRepoStatus: vi.fn(),
  pushRepo: vi.fn(),
}));
vi.mock("@tauri-apps/plugin-fs", () => ({}));
vi.mock("@tauri-apps/api/path", () => ({ join: vi.fn() }));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn() },
}));

const gitService = await import("$lib/services/git");
const { CoreCallError } = await import("$lib/storage/rust-client");
const { SharedRepoManager } = await import("./shared-repo-manager.svelte.js");

const REPO_ID = "repo-1";

function setup() {
  const state = {
    sharedRepos: [{ id: REPO_ID, path: "/repos/team", syncStatus: "synced" } as SharedQueryRepo],
    syncStateByRepo: {},
  } as unknown as DatabaseState;
  const manager = new SharedRepoManager(state, () => {});
  return {
    manager,
    repo: () => state.sharedRepos[0],
    sync: () => state.syncStateByRepo[REPO_ID],
  };
}

function status(overrides: Partial<RepoStatus>): RepoStatus {
  return {
    isClean: true,
    pendingChanges: 0,
    aheadBy: 0,
    behindBy: 0,
    hasConflicts: false,
    currentBranch: "main",
    modifiedFiles: [],
    untrackedFiles: [],
    conflictFiles: [],
    ...overrides,
  };
}

describe("SharedRepoManager and git status", () => {
  beforeEach(() => vi.mocked(gitService.getRepoStatus).mockReset());

  it("lists the conflicted files from the status, not the modified ones", async () => {
    // As after a restart mid-merge: the conflicted file isn't "modified".
    vi.mocked(gitService.getRepoStatus).mockResolvedValue(
      status({
        isClean: false,
        hasConflicts: true,
        aheadBy: 1,
        behindBy: 1,
        modifiedFiles: ["other.sql"],
        pendingChanges: 1,
        conflictFiles: ["q.sql"],
      }),
    );
    const { manager, repo, sync } = setup();
    await manager.refreshRepoStatus(REPO_ID);
    expect(sync().conflictFiles).toEqual(["q.sql"]);
    expect(repo().syncStatus).toBe("diverged");
  });

  it("clears the list when nothing is conflicted", async () => {
    vi.mocked(gitService.getRepoStatus).mockResolvedValue(status({ conflictFiles: [] }));
    const { manager, sync } = setup();
    await manager.refreshRepoStatus(REPO_ID);
    expect(sync().conflictFiles).toEqual([]);
  });
});

describe("SharedRepoManager.pushRepo failures", () => {
  beforeEach(() => {
    vi.mocked(gitService.pushRepo).mockReset();
    vi.mocked(gitService.getRepoStatus).mockResolvedValue(status({}));
  });

  it("marks a non-fast-forward push as behind", async () => {
    vi.mocked(gitService.pushRepo).mockRejectedValue(
      new CoreCallError({
        code: "PUSH_ERROR",
        message:
          "Failed to push: rejected (non-fast-forward): cannot push non-fastforwardable reference",
      }),
    );
    const { manager, repo } = setup();
    await expect(manager.pushRepo(REPO_ID)).rejects.toThrow("rejected (non-fast-forward)");
    expect(repo().syncStatus).toBe("behind");
  });

  it("marks a push the server refused as an error, with its reason", async () => {
    vi.mocked(gitService.pushRepo).mockRejectedValue(
      new CoreCallError({
        code: "PUSH_ERROR",
        message:
          "Failed to push: the remote refused refs/heads/main: protected branch hook declined",
      }),
    );
    const { manager, repo, sync } = setup();
    await expect(manager.pushRepo(REPO_ID)).rejects.toThrow();
    expect(repo().syncStatus).toBe("error");
    expect(sync().lastError).toContain("protected branch hook declined");
    expect(sync().isSyncing).toBe(false);
  });
});
