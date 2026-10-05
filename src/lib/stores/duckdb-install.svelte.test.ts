/**
 * The DuckDB support install dialog's store (desktop DuckDB helper plan,
 * Task 5, Decision 10): the steps (checking, the question, the download,
 * a failure worded by code), one shared request for every connect that
 * meets it, cancel through `duckdb_helper_cancel`, and "Install from a
 * file…". A fake service stands in for the Tauri commands.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn() },
}));

const { DuckdbInstallStore, installFailure, sizeText } = await import("./duckdb-install.svelte");
const { ASSET_NAME, FakeDuckdbInstall, HELPER_SIZE, MISSING, helperError } =
  await import("./duckdb-install-testing");
const { m } = await import("$lib/paraglide/messages.js");

let service: InstanceType<typeof FakeDuckdbInstall>;
let store: InstanceType<typeof DuckdbInstallStore>;

/** Lets the store's awaits run. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

beforeEach(() => {
  service = new FakeDuckdbInstall();
  store = new DuckdbInstallStore(service);
});

describe("sizeText", () => {
  it("says sizes in decimal units, as release pages do", () => {
    expect(sizeText(HELPER_SIZE)).toBe("11.5 MB");
    expect(sizeText(4_200_000)).toBe("4.2 MB");
    expect(sizeText(640_001)).toBe("641 KB");
    expect(sizeText(12)).toBe("12 bytes");
  });
});

describe("the question", () => {
  it("opens checking, then asks with the size and the version", async () => {
    void store.request();
    expect(store.open).toBe(true);
    expect(store.stage.step).toBe("checking");
    await settle();
    expect(store.stage).toEqual({
      step: "ask",
      size: HELPER_SIZE,
      version: "2026.10.1",
      repair: false,
    });
    expect(service.calls).toEqual(["offer"]);
  });

  it("says an unsafe helper's folder is made private again", async () => {
    service.offerAnswer = { ...MISSING, status: "unsafe" };
    void store.request();
    await settle();
    expect(store.stage).toMatchObject({ step: "ask", repair: true });
  });

  it("declining closes the dialog and answers false, downloading nothing", async () => {
    const answer = store.request();
    await settle();
    store.dismiss();
    await expect(answer).resolves.toBe(false);
    expect(store.open).toBe(false);
    expect(service.calls).toEqual(["offer"]);
  });

  it("closing while it checks answers false and ignores the late offer", async () => {
    const answer = store.request();
    store.dismiss();
    await expect(answer).resolves.toBe(false);
    await settle();
    expect(store.open).toBe(false);
  });

  it("a helper installed meanwhile (the prefetch) answers true at once", async () => {
    service.offerAnswer = { ...MISSING, status: "installed", size: null };
    await expect(store.request()).resolves.toBe(true);
    expect(store.open).toBe(false);
  });
});

describe("the download", () => {
  it("moves the bar with the progress and answers true when installed", async () => {
    const answer = store.request();
    await settle();
    store.download();
    expect(store.stage).toEqual({
      step: "downloading",
      bytes: 0,
      total: HELPER_SIZE,
    });
    await service.started();
    service.progress(4_200_000);
    expect(store.stage).toEqual({
      step: "downloading",
      bytes: 4_200_000,
      total: HELPER_SIZE,
    });
    service.finish();
    await expect(answer).resolves.toBe(true);
    expect(store.open).toBe(false);
    expect(service.calls).toEqual(["offer", "install"]);
  });

  it("cancel calls duckdb_helper_cancel, closes and answers false", async () => {
    const answer = store.request();
    await settle();
    store.download();
    await service.started();
    store.dismiss();
    await expect(answer).resolves.toBe(false);
    expect(service.calls).toEqual(["offer", "install", "cancel"]);
    expect(store.open).toBe(false);
  });

  it("an install cancelled elsewhere closes quietly", async () => {
    const answer = store.request();
    await settle();
    store.download();
    await service.started();
    service.fail("CANCELLED");
    await expect(answer).resolves.toBe(false);
    expect(store.open).toBe(false);
  });

  it("two connects share one dialog, one offer and one install", async () => {
    const first = store.request();
    const second = store.request();
    await settle();
    store.download();
    await service.started();
    service.finish();
    await expect(first).resolves.toBe(true);
    await expect(second).resolves.toBe(true);
    expect(service.calls).toEqual(["offer", "install"]);
  });

  it("a request after one ended asks again", async () => {
    const first = store.request();
    await settle();
    store.dismiss();
    await first;
    void store.request();
    await settle();
    expect(store.open).toBe(true);
    expect(service.calls).toEqual(["offer", "offer"]);
  });
});

describe("failures", () => {
  async function failDownload(code: string) {
    const answer = store.request();
    await settle();
    store.download();
    await service.started();
    service.fail(code, "Core's reason");
    await settle();
    // Wrapped: an async function returning the promise would wait for it.
    return { answer };
  }

  it("a download failure is worded by code, with Core's message, and Try again downloads again", async () => {
    const { answer } = await failDownload("DIGEST_MISMATCH");
    expect(store.stage).toEqual({
      step: "failed",
      failure: {
        code: "DIGEST_MISMATCH",
        message: "Core's reason",
        title: m.duckdb_install_damaged_title(),
        hint: m.duckdb_install_damaged_hint(),
        retry: true,
        fromFile: false,
      },
    });
    store.retry();
    expect(store.stage.step).toBe("downloading");
    await service.started();
    service.finish();
    await expect(answer).resolves.toBe(true);
    expect(service.calls).toEqual(["offer", "install", "install"]);
  });

  it("NETWORK_ERROR offers Install from a file…, checked against the built-in digest", async () => {
    const { answer } = await failDownload("NETWORK_ERROR");
    expect(store.stage).toMatchObject({
      step: "failed",
      failure: { code: "NETWORK_ERROR", retry: true, fromFile: true },
    });
    await store.installFromFile();
    expect(store.stage).toEqual({ step: "installingFile" });
    service.finish({ downloaded: true, pruned: 0 });
    await expect(answer).resolves.toBe(true);
    expect(service.calls).toEqual(["offer", "install", "pick", "file /copies/seaquel-duckdb.gz"]);
  });

  // Task 10's P2: under the pin the download is the only request, so a
  // release without this platform's file says so there. The file can still
  // come from another computer.
  it.each(["ASSET_NOT_FOUND", "RELEASE_NOT_FOUND"])(
    "%s offers Install from a file… under the pin, naming the file",
    async (code) => {
      const { answer } = await failDownload(code);
      expect(store.stage).toEqual({
        step: "failed",
        failure: {
          code,
          message: "Core's reason",
          title: m.duckdb_install_unpublished_title(),
          hint: m.duckdb_install_unpublished_hint_file({
            file: ASSET_NAME,
            version: "2026.10.1",
          }),
          retry: true,
          fromFile: true,
        },
      });
      expect(
        m.duckdb_install_unpublished_hint_file({
          file: ASSET_NAME,
          version: "2026.10.1",
        }),
      ).toContain(`${ASSET_NAME} from the v2026.10.1 release`);
      await store.installFromFile();
      service.finish({ downloaded: true, pruned: 0 });
      await expect(answer).resolves.toBe(true);
      expect(service.calls).toEqual(["offer", "install", "pick", "file /copies/seaquel-duckdb.gz"]);
    },
  );

  it.each(["ASSET_NOT_FOUND", "RELEASE_NOT_FOUND"])(
    "%s without the pin offers no file and keeps the plain hint",
    async (code) => {
      service.offerAnswer = { ...MISSING, fromFile: false };
      await failDownload(code);
      expect(store.stage).toMatchObject({
        failure: {
          code,
          fromFile: false,
          hint: m.duckdb_install_unpublished_hint(),
        },
      });
    },
  );

  it("an unusable answer (HTTP_ERROR) still offers no file", async () => {
    await failDownload("HTTP_ERROR");
    expect(store.stage).toMatchObject({
      failure: {
        code: "HTTP_ERROR",
        fromFile: false,
        hint: m.duckdb_install_metadata_hint(),
      },
    });
  });

  it("no file picker without a built-in digest", async () => {
    service.offerAnswer = { ...MISSING, fromFile: false };
    await failDownload("NETWORK_ERROR");
    expect(store.stage).toMatchObject({ failure: { fromFile: false } });
  });

  it("a cancelled file picker changes nothing", async () => {
    await failDownload("NETWORK_ERROR");
    service.picked = null;
    await store.installFromFile();
    expect(store.stage).toMatchObject({
      step: "failed",
      failure: { code: "NETWORK_ERROR" },
    });
    expect(service.calls).toEqual(["offer", "install", "pick"]);
  });

  it("the wrong file says which file to pick, and offers the picker again", async () => {
    await failDownload("NETWORK_ERROR");
    await store.installFromFile();
    service.fail("WRONG_FILE", "This isn't seaquel-duckdb-x.gz from the v2026.10.1 release.");
    await settle();
    expect(store.stage).toEqual({
      step: "failed",
      failure: {
        code: "WRONG_FILE",
        message: "This isn't seaquel-duckdb-x.gz from the v2026.10.1 release.",
        title: m.duckdb_install_wrong_file_title(),
        hint: m.duckdb_install_wrong_file_hint(),
        retry: true,
        fromFile: true,
      },
    });
  });

  it("Try again after a file failure downloads, when the size is known (review M5)", async () => {
    const { answer } = await failDownload("NETWORK_ERROR");
    await store.installFromFile();
    service.fail("DIGEST_MISMATCH");
    await settle();
    expect(store.stage).toMatchObject({
      failure: { retry: true, fromFile: true },
    });
    store.retry();
    expect(store.stage.step).toBe("downloading");
    await service.started();
    service.finish();
    await expect(answer).resolves.toBe(true);
    expect(service.calls.filter((c) => c === "install")).toHaveLength(2);
  });

  it("the network failure names the file to copy and its release (review M7)", async () => {
    await failDownload("NETWORK_ERROR");
    expect(store.stage).toMatchObject({
      failure: {
        hint: m.duckdb_install_network_hint_file({
          file: ASSET_NAME,
          version: "2026.10.1",
        }),
      },
    });
    expect(
      m.duckdb_install_network_hint_file({
        file: ASSET_NAME,
        version: "2026.10.1",
      }),
    ).toContain(`${ASSET_NAME} from the v2026.10.1 release`);
  });

  it("without the file picker the network failure doesn't mention it", async () => {
    service.offerAnswer = { ...MISSING, fromFile: false };
    await failDownload("NETWORK_ERROR");
    expect(store.stage).toMatchObject({
      failure: { hint: m.duckdb_install_network_hint() },
    });
  });

  it("NOT_SUPPORTED has no Try again", async () => {
    await failDownload("NOT_SUPPORTED");
    expect(store.stage).toMatchObject({
      failure: { code: "NOT_SUPPORTED", retry: false, fromFile: false },
    });
  });

  it("a size that couldn't be had fails the check, and Try again checks again", async () => {
    service.offerAnswer = {
      ...MISSING,
      size: null,
      sizeError: { code: "NETWORK_ERROR", message: "no route" },
    };
    void store.request();
    await settle();
    expect(store.stage).toMatchObject({
      step: "failed",
      failure: {
        code: "NETWORK_ERROR",
        message: "no route",
        retry: true,
        fromFile: true,
      },
    });
    service.offerAnswer = MISSING;
    store.retry();
    await settle();
    expect(store.stage.step).toBe("ask");
    expect(service.calls).toEqual(["offer", "offer"]);
  });

  it("an offer that fails outright is a failure too", async () => {
    service.offerAnswer = helperError("NOT_SUPPORTED", "no locator");
    void store.request();
    await settle();
    expect(store.stage).toMatchObject({
      step: "failed",
      failure: { code: "NOT_SUPPORTED", retry: false },
    });
  });

  it("closing a failure answers false", async () => {
    const { answer } = await failDownload("FILE_ERROR");
    store.dismiss();
    await expect(answer).resolves.toBe(false);
    expect(store.open).toBe(false);
  });
});

describe("installFailure", () => {
  const cases: Array<[string, string, () => string, () => string]> = [
    ["NETWORK_ERROR", "download", m.duckdb_install_network_title, m.duckdb_install_network_hint],
    [
      "RELEASE_NOT_FOUND",
      "download",
      m.duckdb_install_unpublished_title,
      m.duckdb_install_unpublished_hint,
    ],
    [
      "ASSET_NOT_FOUND",
      "check",
      m.duckdb_install_unpublished_title,
      m.duckdb_install_unpublished_hint,
    ],
    ["SIZE_MISMATCH", "download", m.duckdb_install_damaged_title, m.duckdb_install_damaged_hint],
    ["GZIP_ERROR", "download", m.duckdb_install_damaged_title, m.duckdb_install_damaged_hint],
    [
      "DIGEST_MISMATCH",
      "file",
      m.duckdb_install_file_damaged_title,
      m.duckdb_install_file_damaged_hint,
    ],
    [
      "RELEASE_METADATA_INVALID",
      "check",
      m.duckdb_install_metadata_title,
      m.duckdb_install_metadata_hint,
    ],
    [
      "REDIRECT_REFUSED",
      "download",
      m.duckdb_install_metadata_title,
      m.duckdb_install_metadata_hint,
    ],
    ["FILE_ERROR", "download", m.duckdb_install_disk_title, m.duckdb_install_disk_hint],
    [
      "UNSAFE_FOLDER",
      "download",
      m.duckdb_install_unsafe_folder_title,
      m.duckdb_install_unsafe_folder_hint,
    ],
    [
      "NOT_SUPPORTED",
      "download",
      m.duckdb_install_not_supported_title,
      m.duckdb_install_not_supported_hint,
    ],
    ["WRONG_FILE", "file", m.duckdb_install_wrong_file_title, m.duckdb_install_wrong_file_hint],
    ["SOMETHING_NEW", "download", m.duckdb_install_other_title, m.duckdb_install_other_hint],
  ];
  it.each(cases)("%s (%s)", (code, step, title, hint) => {
    expect(installFailure(code, step as "check" | "download" | "file")).toEqual({
      title: title(),
      hint: hint(),
    });
  });
});

describe("after an install", () => {
  it("shows that DuckDB support can't be used, with Core's reason and no download", async () => {
    store.showUnusable("the helper answered another version");
    expect(store.open).toBe(true);
    expect(store.stage).toEqual({
      step: "unusable",
      reason: "the helper answered another version",
    });
    store.dismiss();
    expect(store.open).toBe(false);
    expect(service.calls).toEqual([]);
  });

  it("a request while it shows answers at once, with no new step (review M6)", async () => {
    store.showUnusable("the helper answered another version");
    await expect(store.request()).resolves.toBe(true);
    expect(store.open).toBe(true);
    expect(store.stage).toEqual({
      step: "unusable",
      reason: "the helper answered another version",
    });
    expect(service.calls).toEqual([]);
  });
});

describe("the prefetch (Task 6, Decision 11)", () => {
  /** Every line the store logged, as text. */
  async function logged(): Promise<string[]> {
    const { log } = await import("$lib/utils/logger");
    return (["info", "warn", "error", "debug"] as const).flatMap((level) =>
      vi.mocked(log[level]).mock.calls.map((args) => args.map(String).join(" ")),
    );
  }

  beforeEach(async () => {
    const { log } = await import("$lib/utils/logger");
    for (const level of ["info", "warn", "error", "debug"] as const)
      vi.mocked(log[level]).mockClear();
  });

  it("installs a missing helper silently, without listening to its progress", async () => {
    const prefetched = store.prefetch();
    await service.started();
    expect(service.calls).toEqual(["offer", "install"]);
    expect(service.listened).toEqual([false]);
    expect(store.open).toBe(false);
    service.finish();
    await expect(prefetched).resolves.toBe("installed");
    expect(store.open).toBe(false);
  });

  it("does nothing for a helper that is already installed", async () => {
    service.offerAnswer = {
      ...MISSING,
      status: "installed",
      size: null,
      assetName: null,
    };
    await expect(store.prefetch()).resolves.toBe("present");
    expect(service.calls).toEqual(["offer"]);
  });

  it("repairs an outdated or unsafe helper too", async () => {
    service.offerAnswer = { ...MISSING, status: "unsafe" };
    const prefetched = store.prefetch();
    await service.started();
    service.finish();
    await expect(prefetched).resolves.toBe("installed");
  });

  it("downloads nothing in a build without the built-in digest", async () => {
    service.offerAnswer = { ...MISSING, fromFile: false };
    await expect(store.prefetch()).resolves.toBe("unpinned");
    expect(service.calls).toEqual(["offer"]);
  });

  it("runs once per page", async () => {
    service.offerAnswer = {
      ...MISSING,
      status: "installed",
      size: null,
      assetName: null,
    };
    await store.prefetch();
    await expect(store.prefetch()).resolves.toBe("repeated");
    expect(service.calls).toEqual(["offer"]);
  });

  it("a failure is silent: no dialog, a log line with the code only", async () => {
    const prefetched = store.prefetch();
    await service.started();
    service.fail("NETWORK_ERROR", "couldn't reach the server at https://example.invalid/x");
    await expect(prefetched).resolves.toBe("failed");
    expect(store.open).toBe(false);
    const lines = await logged();
    expect(lines.some((line) => line.includes("NETWORK_ERROR"))).toBe(true);
    expect(lines.some((line) => line.includes("example.invalid"))).toBe(false);
    // Once per page: a failure isn't tried again until the next start.
    await expect(store.prefetch()).resolves.toBe("repeated");
  });

  it("an offer that fails is silent too", async () => {
    service.offerAnswer = helperError("NOT_SUPPORTED", "no locator");
    await expect(store.prefetch()).resolves.toBe("failed");
    expect(store.open).toBe(false);
    expect(service.calls).toEqual(["offer"]);
  });

  it("leaves it to the dialog when the dialog is open", async () => {
    void store.request();
    await settle();
    expect(store.stage.step).toBe("ask");
    await expect(store.prefetch()).resolves.toBe("dialog");
    expect(service.calls).toEqual(["offer"]);
    expect(store.stage.step).toBe("ask");
  });

  it("leaves it to the dialog when the dialog opened while the prefetch checked", async () => {
    const answers: Array<(offer: typeof MISSING) => void> = [];
    service.offer = () => {
      service.calls.push("offer");
      return new Promise((resolve) => answers.push(resolve));
    };
    const prefetched = store.prefetch();
    await settle();
    void store.request();
    for (const answer of answers) answer(MISSING);
    await expect(prefetched).resolves.toBe("dialog");
    expect(service.downloads).toBe(0);
  });

  it("doesn't download after the user said Not now this session", async () => {
    void store.request();
    await settle();
    store.dismiss();
    service.calls.length = 0;
    await expect(store.prefetch()).resolves.toBe("declined");
    expect(service.calls).toEqual([]);
  });

  it("a dialog opened during the prefetch joins its download instead of asking", async () => {
    const prefetched = store.prefetch();
    await service.started();
    service.progress(4_200_000);
    const requested = store.request();
    await settle();
    expect(store.stage).toEqual({
      step: "downloading",
      bytes: 4_200_000,
      total: HELPER_SIZE,
    });
    service.progress(8_000_000);
    expect(store.stage).toEqual({
      step: "downloading",
      bytes: 8_000_000,
      total: HELPER_SIZE,
    });
    service.finish();
    await expect(requested).resolves.toBe(true);
    await expect(prefetched).resolves.toBe("installed");
    expect(service.downloads).toBe(1);
    expect(store.open).toBe(false);
  });

  it("a joined dialog shows the download starting until progress reaches it", async () => {
    const prefetched = store.prefetch();
    await service.started();
    void store.request();
    await settle();
    expect(store.stage).toEqual({
      step: "downloading",
      bytes: 0,
      total: HELPER_SIZE,
    });
    service.finish();
    await prefetched;
  });

  it("Cancel in a joined dialog stops the prefetch's download", async () => {
    const prefetched = store.prefetch();
    await service.started();
    const requested = store.request();
    await settle();
    expect(store.stage.step).toBe("downloading");
    store.dismiss();
    await expect(requested).resolves.toBe(false);
    await expect(prefetched).resolves.toBe("cancelled");
    expect(service.calls).toContain("cancel");
    expect(service.busy).toBe(false);
  });

  it("a dialog after the prefetch failed asks as usual", async () => {
    const prefetched = store.prefetch();
    await service.started();
    service.fail("NETWORK_ERROR");
    await prefetched;
    void store.request();
    await settle();
    expect(store.stage.step).toBe("ask");
  });
});

describe("installJoined (the MCP panel's install, Task 6 review)", () => {
  it("installs with progress, and a dialog opened meanwhile joins it", async () => {
    const seen: number[] = [];
    const installed = store.installJoined((p) => seen.push(p.bytes));
    await service.started();
    service.progress(4_200_000);
    expect(seen).toEqual([4_200_000]);
    const requested = store.request();
    await settle();
    expect(store.stage).toEqual({
      step: "downloading",
      bytes: 4_200_000,
      total: HELPER_SIZE,
    });
    service.finish();
    await expect(installed).resolves.toEqual({ downloaded: true, pruned: 0 });
    await expect(requested).resolves.toBe(true);
    expect(service.downloads).toBe(1);
  });

  it("rejects with the install's error, and no longer counts as running", async () => {
    const installed = store.installJoined();
    await service.started();
    service.fail("NETWORK_ERROR");
    await expect(installed).rejects.toMatchObject({ code: "NETWORK_ERROR" });
    void store.request();
    await settle();
    expect(store.stage.step).toBe("ask");
  });

  it("a joined dialog's Cancel stops it with CANCELLED", async () => {
    const installed = store.installJoined();
    await service.started();
    void store.request();
    await settle();
    store.dismiss();
    await expect(installed).rejects.toMatchObject({ code: "CANCELLED" });
  });
});
