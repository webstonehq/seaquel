/**
 * Deep links to shared files (phase 5e Task 7): a query or dashboard opens
 * the stored row whose `sharedPath` is the link's path, after its project
 * synced; a file that isn't stored yet says so (bug 24) instead of opening a
 * tab on a scan id.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";

const toasts = vi.hoisted(() => ({ errors: [] as string[], other: [] as string[] }));
vi.mock("$lib/utils/toast", () => ({ errorToast: (m: string) => toasts.errors.push(m) }));
vi.mock("svelte-sonner", () => ({
  toast: {
    success: (m: string) => toasts.other.push(m),
    info: (m: string) => toasts.other.push(m),
  },
}));
vi.mock("$lib/stores/deep-link-dialog.svelte.js", () => ({
  deepLinkDialogStore: { prompt: vi.fn(async () => false) },
}));

const { handleDeepLink, parseSettingsLink } = await import("./deep-link");
const { importSelectedProjects } = await import("./shared-project-import");
const { sharedProjectImportStore } = await import("$lib/stores/shared-project-import.svelte.js");
const { m } = await import("$lib/paraglide/messages.js");

const REMOTE = "https://github.com/acme/team";
const PATH = ".seaquel/projects/team/queries/orders.sql";
const TEMPLATE = ".seaquel/projects/team/connections/warehouse.yaml";
const link = (path: string) => `seaquel://open?r=${REMOTE}/blob/main/${path}`;

function fakeDb({ linked = true } = {}) {
  const calls: unknown[][] = [];
  const state = {
    sharedRepos: [
      { id: "repo-1", path: "/repos/team", remoteUrl: `${REMOTE}.git`, branch: "main" },
    ],
    projects: [{ id: "p1", name: "Team", gitRepoPath: "/repos/team" }],
    activeProjectId: "p1",
    queriesByProject: {
      p1: [{ id: "saved-1", name: "Orders", shared: true, sharedPath: PATH }],
    } as Record<string, { id: string; name: string; shared: boolean; sharedPath?: string }[]>,
    dashboardsByProject: { p1: [] } as Record<string, unknown[]>,
    connections: [] as {
      id: string;
      name: string;
      projectId: string;
      sharedConnectionId?: string;
    }[],
  };
  const db = {
    state,
    whenReady: async () => {},
    sharedRepos: {
      scan: vi.fn(async () => ({
        conflicted: false,
        projects: [
          {
            dir: "team",
            name: "Team",
            queries: 1,
            dashboards: 0,
            templates: [],
            skipped: 0,
            linkedProjectIds: linked ? ["p1"] : [],
          },
        ],
      })),
      syncProject: vi.fn(async (id: string) => {
        calls.push(["sync", id]);
        return null;
      }),
    },
    projects: {
      setActive: vi.fn(async () => {}),
      importProjects: vi.fn(async (_path: string, dirs: string[]) => {
        calls.push(["import", ...dirs]);
        // As Core imports the directory's templates into the new project.
        state.projects.push({ id: "p9", name: "Team", gitRepoPath: "/repos/team" });
        state.connections.push({
          id: "conn-9",
          name: "Warehouse",
          projectId: "p9",
          sharedConnectionId: `repo-1:${TEMPLATE}`,
        });
        return ["p9"];
      }),
    },
    connectionTabs: {
      open: vi.fn(async (c: { id: string }) => {
        calls.push(["openConnection", c.id]);
        return "tab";
      }),
    },
    queryTabs: { loadQuery: vi.fn((id: string) => calls.push(["open", id])) },
    dashboardTabs: { add: vi.fn() },
    settingsTabs: { open: vi.fn(() => "settings") },
    ui: { setActiveView: vi.fn() },
  };
  return { db, calls };
}

beforeEach(() => {
  toasts.errors.length = 0;
  toasts.other.length = 0;
});

describe("query deep links", () => {
  it("a deep link to a stored shared query opens it by path", async () => {
    const { db, calls } = fakeDb();
    await handleDeepLink(link(PATH), db as never);
    expect(calls).toEqual([
      ["sync", "p1"],
      ["open", "saved-1"],
    ]);
    expect(toasts.errors).toEqual([]);
  });

  it("a file that isn't stored yet says so and opens nothing", async () => {
    const { db, calls } = fakeDb();
    const other = ".seaquel/projects/team/queries/new.sql";
    await handleDeepLink(link(other), db as never);
    expect(calls).toEqual([["sync", "p1"]]);
    expect(toasts.errors).toEqual([m.deep_link_not_stored({ path: other })]);
  });
});

describe("connection deep links", () => {
  beforeEach(() => sharedProjectImportStore.reset());

  it("in a linked project, opens the connection whose template is the link's path", async () => {
    const { db, calls } = fakeDb();
    db.state.connections.push(
      {
        id: "conn-other",
        name: "Other",
        projectId: "p1",
        sharedConnectionId: "repo-1:.seaquel/projects/team/connections/other.yaml",
      },
      {
        id: "conn-1",
        name: "Warehouse",
        projectId: "p1",
        sharedConnectionId: `repo-1:${TEMPLATE}`,
      },
    );
    await handleDeepLink(link(TEMPLATE), db as never);
    expect(calls).toEqual([
      ["sync", "p1"],
      ["openConnection", "conn-1"],
    ]);
  });

  it("in a project no one links here, asks to import it, then opens the connection", async () => {
    const { db, calls } = fakeDb({ linked: false });
    await handleDeepLink(link(TEMPLATE), db as never);

    // Nothing imported yet: the import dialog asks, with the directory ticked.
    expect(calls).toEqual([]);
    expect(sharedProjectImportStore.isOpen).toBe(true);
    expect(sharedProjectImportStore.folderPath).toBe("/repos/team");
    expect(sharedProjectImportStore.discoveredProjects.map((p) => [p.dir, p.selected])).toEqual([
      ["team", true],
    ]);

    await importSelectedProjects(db as never);

    expect(calls).toEqual([
      ["import", "team"],
      ["openConnection", "conn-9"],
    ]);
    expect(sharedProjectImportStore.isOpen).toBe(false);
  });

  it("cancelling the import does nothing", async () => {
    const { db, calls } = fakeDb({ linked: false });
    await handleDeepLink(link(TEMPLATE), db as never);
    sharedProjectImportStore.reset();
    await importSelectedProjects(db as never);
    expect(calls).toEqual([]);
    expect(db.projects.importProjects).not.toHaveBeenCalled();
  });
});

describe("settings deep links", () => {
  it("parses a known app settings section", () => {
    expect(parseSettingsLink("seaquel://settings/updates")).toBe("updates");
    expect(parseSettingsLink("seaquel://settings/license/")).toBe("license");
  });

  it("ignores other links, unknown sections and extra path", () => {
    expect(parseSettingsLink(link(PATH))).toBeNull();
    expect(parseSettingsLink("seaquel://settings")).toBeNull();
    expect(parseSettingsLink("seaquel://settings/nope")).toBeNull();
    expect(parseSettingsLink("seaquel://settings/updates/more")).toBeNull();
    expect(parseSettingsLink("seaquel://settings/constructor")).toBeNull();
    expect(parseSettingsLink("https://seaquel.app/settings/updates")).toBeNull();
  });

  it("opens app settings at the section and nothing else", async () => {
    const { db, calls } = fakeDb();
    await handleDeepLink("seaquel://settings/updates", db as never);
    expect(db.settingsTabs.open).toHaveBeenCalledWith("app", "updates");
    expect(calls).toEqual([]);
    expect(db.sharedRepos.scan).not.toHaveBeenCalled();
  });

  it("an unknown section does nothing", async () => {
    const { db } = fakeDb();
    await handleDeepLink("seaquel://settings/nope", db as never);
    expect(db.settingsTabs.open).not.toHaveBeenCalled();
    expect(toasts.errors).toEqual([]);
  });
});
