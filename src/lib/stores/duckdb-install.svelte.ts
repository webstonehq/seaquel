/**
 * The DuckDB support install dialog's state (desktop DuckDB helper plan,
 * Task 5, Decision 10; the TUI's `state/install.rs` in the GUI's shape).
 *
 * `withDuckdbHelper` (`$lib/core/duckdb-helper`) calls `request()` when a
 * connect answers `ENGINE_NOT_INSTALLED`, and connects again with the same
 * request once it answers `true`. Every connect that meets it while the
 * dialog is open shares the one request, so two dropped files get one
 * dialog and one download. The steps: checking (the offer: status and
 * size; no network when the digest is built in), the question, the
 * download with its progress, and a failure worded by code with Try again
 * and, for network failures, "Install from a file…" (a copy of the
 * release's `.gz`, which Core checks against the built-in digest).
 *
 * The prefetch (Task 6, Decision 11) lives here too, so the dialog knows
 * about it: at startup, when a saved connection is DuckDB, `prefetch()`
 * installs a missing helper without asking, once per page. A dialog that
 * opens meanwhile joins that download (Core's one install) instead of
 * asking, and its Cancel stops it.
 *
 * Desktop only: the layout mounts the dialog behind the build constant,
 * and the wrapper imports this store only in a desktop build.
 */

import {
  cancelDuckdbHelperInstall,
  duckdbHelperOffer,
  installDuckdbHelper,
  installDuckdbHelperFromFile,
  type DuckdbHelperInstalled,
  type DuckdbHelperOffer,
  type DuckdbHelperProgress,
} from "$lib/api/tauri";
import { m } from "$lib/paraglide/messages.js";
import { log } from "$lib/utils/logger";

export { sizeText } from "$lib/utils/size-text";

/** The commands the dialog uses (`$lib/api/tauri` in the app, a fake in tests). */
export interface DuckdbInstallService {
  offer(): Promise<DuckdbHelperOffer>;
  install(onProgress?: (progress: DuckdbHelperProgress) => void): Promise<DuckdbHelperInstalled>;
  cancel(): Promise<boolean>;
  /** The OS file picker; `null` when the user closed it. */
  pickFile(): Promise<string | null>;
  installFromFile(path: string): Promise<DuckdbHelperInstalled>;
}

/** Which step a failure belongs to: Try again repeats it. */
export type InstallStep = "check" | "download" | "file";

export interface InstallFailure {
  code: string;
  /** Core's message: it names no path or URL. */
  message: string;
  title: string;
  hint: string;
  /** Whether Try again is offered (never for `NOT_SUPPORTED` or a file). */
  retry: boolean;
  /** Whether "Install from a file…" is offered. */
  fromFile: boolean;
}

export type InstallStage =
  | { step: "checking" }
  | { step: "ask"; size: number; version: string; repair: boolean }
  | { step: "downloading"; bytes: number; total: number }
  | { step: "installingFile" }
  | { step: "failed"; failure: InstallFailure }
  | { step: "unusable"; reason: string };

const METADATA_CODES = new Set([
  "DIGEST_MISSING",
  "DIGEST_INVALID",
  "SIZE_INVALID",
  "ASSET_TOO_LARGE",
  "RELEASE_METADATA_INVALID",
  "HTTP_ERROR",
  "REDIRECT_REFUSED",
]);
const DAMAGED_CODES = new Set(["DIGEST_MISMATCH", "SIZE_MISMATCH", "GZIP_ERROR"]);
/**
 * Failures a copy of the file from another computer gets past, so the file
 * picker is offered for them (when the build has the digest to check it
 * against): no network, and, under the pin, a release that lacks the file
 * (the download is the only request, so a 404 there is `ASSET_NOT_FOUND`;
 * Task 10's P2).
 */
const FILE_CODES = new Set(["NETWORK_ERROR", "ASSET_NOT_FOUND", "RELEASE_NOT_FOUND"]);

/**
 * A failed step's title and what to do about it. With `copy` (the file
 * picker is offered and the asset's name is known), a network failure or
 * an unpublished file names the file to copy from another computer.
 */
export function installFailure(
  code: string,
  step: InstallStep,
  copy?: { file: string; version: string },
): { title: string; hint: string } {
  if (code === "WRONG_FILE") {
    return {
      title: m.duckdb_install_wrong_file_title(),
      hint: m.duckdb_install_wrong_file_hint(),
    };
  }
  if (DAMAGED_CODES.has(code)) {
    return step === "file"
      ? {
          title: m.duckdb_install_file_damaged_title(),
          hint: m.duckdb_install_file_damaged_hint(),
        }
      : {
          title: m.duckdb_install_damaged_title(),
          hint: m.duckdb_install_damaged_hint(),
        };
  }
  if (METADATA_CODES.has(code)) {
    return {
      title: m.duckdb_install_metadata_title(),
      hint: m.duckdb_install_metadata_hint(),
    };
  }
  switch (code) {
    case "NETWORK_ERROR":
      return {
        title: m.duckdb_install_network_title(),
        hint: copy ? m.duckdb_install_network_hint_file(copy) : m.duckdb_install_network_hint(),
      };
    case "RELEASE_NOT_FOUND":
    case "ASSET_NOT_FOUND":
      return {
        title: m.duckdb_install_unpublished_title(),
        hint: copy
          ? m.duckdb_install_unpublished_hint_file(copy)
          : m.duckdb_install_unpublished_hint(),
      };
    case "FILE_ERROR":
      return {
        title: m.duckdb_install_disk_title(),
        hint: m.duckdb_install_disk_hint(),
      };
    case "UNSAFE_FOLDER":
      return {
        title: m.duckdb_install_unsafe_folder_title(),
        hint: m.duckdb_install_unsafe_folder_hint(),
      };
    case "NOT_SUPPORTED":
      return {
        title: m.duckdb_install_not_supported_title(),
        hint: m.duckdb_install_not_supported_hint(),
      };
    default:
      return {
        title: m.duckdb_install_other_title(),
        hint: m.duckdb_install_other_hint(),
      };
  }
}

/** A rejected command's code and message. */
function codeOf(error: unknown): { code: string; message: string } {
  if (typeof error === "object" && error !== null) {
    const { code, message } = error as { code?: unknown; message?: unknown };
    if (typeof code === "string") {
      return { code, message: typeof message === "string" ? message : "" };
    }
  }
  return {
    code: "UNKNOWN",
    message: error instanceof Error ? error.message : String(error),
  };
}

/** The app's commands, and the dialog plugin's picker. */
const tauriService: DuckdbInstallService = {
  offer: duckdbHelperOffer,
  install: installDuckdbHelper,
  cancel: cancelDuckdbHelperInstall,
  installFromFile: installDuckdbHelperFromFile,
  async pickFile() {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const picked = await open({
      multiple: false,
      directory: false,
      title: m.duckdb_install_pick_title(),
      filters: [{ name: m.duckdb_install_file_filter(), extensions: ["gz"] }],
    });
    return typeof picked === "string" ? picked : null;
  },
};

/**
 * How a prefetch ended (logged; the tests read it): `installed`, `present`
 * (nothing to do), `unpinned` (no built-in digest: a debug or local build,
 * which never downloads unasked), `repeated` (it already ran on this
 * page), `dialog` (the dialog is asking instead), `declined` (the user
 * said Not now or cancelled this session), `cancelled` (stopped through a
 * dialog that joined it) or `failed` (logged by code; the dialog covers it
 * at connect time).
 */
export type PrefetchOutcome =
  | "installed"
  | "present"
  | "unpinned"
  | "repeated"
  | "dialog"
  | "declined"
  | "cancelled"
  | "failed";

export class DuckdbInstallStore {
  open = $state(false);
  stage = $state<InstallStage>({ step: "checking" });

  /** The request every connect that meets the dialog shares. */
  private pending: Promise<boolean> | null = null;
  private resolvePending: ((installed: boolean) => void) | null = null;
  /** What the offer said; kept for the download's total and the picker. */
  private offer: DuckdbHelperOffer | null = null;
  /** Bumped whenever the dialog moves on: a late answer to an older step is dropped. */
  private generation = 0;
  /** The step Try again repeats. */
  private failedStep: InstallStep = "check";
  /** Whether the prefetch already ran on this page. */
  private prefetched = false;
  /** Installs running without the dialog (the prefetch, the MCP panel): a dialog joins them. */
  private joinable = 0;
  /** The user closed the dialog this session: no download unasked. */
  private declined = false;

  constructor(private readonly service: DuckdbInstallService = tauriService) {}

  /**
   * Open the dialog (or join the one already open). Resolves `true` once
   * DuckDB support is installed, `false` when the user declined or
   * cancelled, or the dialog was closed on a failure.
   */
  request(): Promise<boolean> {
    if (this.pending) return this.pending;
    // "Installed but can't be used" is on screen: there is nothing to
    // download, so the connect goes again at once (and, refused again,
    // keeps this step; no flicker through checking).
    if (this.open && this.stage.step === "unusable") return Promise.resolve(true);
    this.pending = new Promise<boolean>((resolve) => {
      this.resolvePending = resolve;
    });
    this.open = true;
    void this.check();
    return this.pending;
  }

  /** "Download" on the question, from its offer. */
  download(): void {
    if (this.stage.step !== "ask" && this.stage.step !== "failed") return;
    this.startDownload();
  }

  /**
   * Install in the background, without asking (Decision 11): a missing,
   * outdated or unsafe helper, in a build with the digest built in, once
   * per page, unless the dialog is asking or the user closed it this
   * session. Never opens the dialog, never throws; logs codes only.
   */
  async prefetch(): Promise<PrefetchOutcome> {
    const outcome = await this.runPrefetch();
    void log.info(`DuckDB support: prefetch ${outcome}`);
    return outcome;
  }

  private async runPrefetch(): Promise<PrefetchOutcome> {
    if (this.prefetched) return "repeated";
    if (this.pending || this.open) return "dialog";
    if (this.declined) return "declined";
    this.prefetched = true;
    let offer: DuckdbHelperOffer;
    try {
      offer = await this.service.offer();
    } catch (error) {
      void log.warn(`DuckDB support: prefetch check failed (${codeOf(error).code})`);
      return "failed";
    }
    if (offer.status === "installed") return "present";
    if (!offer.fromFile) return "unpinned";
    // The dialog opened while the offer was asked: it asks the user.
    if (this.pending || this.open) return "dialog";
    if (this.declined) return "declined";
    try {
      await this.installJoined();
      return "installed";
    } catch (error) {
      const { code } = codeOf(error);
      if (code === "CANCELLED") return "cancelled";
      void log.warn(`DuckDB support: prefetch failed (${code})`);
      return "failed";
    }
  }

  /**
   * Install without the dialog (the MCP panel's button, the prefetch),
   * where a dialog opened meanwhile goes straight to this download (Core
   * runs one) instead of asking. Rejects with the command's error;
   * `CANCELLED` when a joined dialog's Cancel stopped it.
   */
  async installJoined(
    onProgress?: (progress: DuckdbHelperProgress) => void,
  ): Promise<DuckdbHelperInstalled> {
    this.joinable++;
    try {
      return await this.service.install(onProgress);
    } finally {
      this.joinable--;
    }
  }

  private startDownload(): void {
    const generation = ++this.generation;
    const total = this.offer?.size ?? 0;
    this.stage = { step: "downloading", bytes: 0, total };
    void log.info("DuckDB support: downloading");
    this.service
      .install((progress) => {
        if (generation !== this.generation || this.stage.step !== "downloading") return;
        this.stage = {
          step: "downloading",
          bytes: progress.bytes,
          total: progress.total,
        };
      })
      .then(
        (installed) => {
          if (generation === this.generation) this.installed(installed);
        },
        (error) => {
          if (generation === this.generation) this.failed(error, "download");
        },
      );
  }

  /** Try again: repeat the step that failed. */
  retry(): void {
    if (this.stage.step !== "failed" || !this.stage.failure.retry) return;
    if (this.failedStep === "download") this.download();
    else if (this.failedStep === "check") void this.check();
  }

  /** "Install from a file…": pick the release's `.gz`; Core checks it against the built-in digest. */
  async installFromFile(): Promise<void> {
    if (this.stage.step !== "failed" || !this.stage.failure.fromFile) return;
    const generation = this.generation;
    let path: string | null;
    try {
      path = await this.service.pickFile();
    } catch (error) {
      void log.warn(`DuckDB support: the file picker failed (${codeOf(error).code})`);
      return;
    }
    // Closed meanwhile, or the picker was closed: nothing changes.
    if (generation !== this.generation || path === null) return;
    const fileGeneration = ++this.generation;
    this.stage = { step: "installingFile" };
    void log.info("DuckDB support: installing from a file");
    this.service.installFromFile(path).then(
      (installed) => {
        if (fileGeneration === this.generation) this.installed(installed);
      },
      (error) => {
        if (fileGeneration === this.generation) this.failed(error, "file");
      },
    );
  }

  /**
   * Not now, Cancel, Close or Esc, at any step: a download or file install
   * in flight is cancelled through `duckdb_helper_cancel` (which drops
   * Core's install, so no `.part` is left), and the request answers false.
   */
  dismiss(): void {
    const step = this.stage.step;
    this.generation++;
    this.declined = true;
    if (step === "downloading" || step === "installingFile") {
      void log.info("DuckDB support: install cancelled");
      this.service.cancel().catch((error) => {
        void log.warn(`DuckDB support: cancel failed (${codeOf(error).code})`);
      });
    }
    this.finish(false);
  }

  /**
   * A connect right after an install still answered `ENGINE_NOT_INSTALLED`:
   * say so with Core's reason, and offer no second download (the TUI's
   * `after_install` rule). A dialog already asking for another connect is
   * left as it is.
   */
  showUnusable(reason: string): void {
    if (this.pending) return;
    this.generation++;
    this.stage = { step: "unusable", reason };
    this.open = true;
  }

  private async check(): Promise<void> {
    const generation = ++this.generation;
    this.stage = { step: "checking" };
    let offer: DuckdbHelperOffer;
    try {
      offer = await this.service.offer();
    } catch (error) {
      if (generation === this.generation) this.failed(error, "check");
      return;
    }
    if (generation !== this.generation) return;
    this.offer = offer;
    // Installed meanwhile (the prefetch, another window's install): connect.
    if (offer.status === "installed") {
      this.finish(true);
      return;
    }
    // The prefetch or the MCP panel is downloading it: join that download
    // (Core runs one) rather than ask; the bar starts when its progress
    // reaches us.
    if (this.joinable > 0) {
      void log.info("DuckDB support: joining the running install");
      this.startDownload();
      return;
    }
    if (offer.size === null) {
      this.failed(
        offer.sizeError ?? {
          code: "UNKNOWN",
          message: "the download's size is unknown",
        },
        "check",
      );
      return;
    }
    this.stage = {
      step: "ask",
      size: offer.size,
      version: offer.version,
      repair: offer.status === "unsafe",
    };
  }

  private installed(installed: DuckdbHelperInstalled): void {
    void log.info(
      `DuckDB support: installed (downloaded=${installed.downloaded}, pruned=${installed.pruned})`,
    );
    this.finish(true);
  }

  private failed(error: unknown, step: InstallStep): void {
    const { code, message } = codeOf(error);
    void log.warn(`DuckDB support: ${step} failed (${code})`);
    // Cancelled (by this dialog, or by another caller of the shared
    // install): closed quietly.
    if (code === "CANCELLED") {
      this.finish(false);
      return;
    }
    const offer = this.offer;
    // After a file failure, Try again downloads when the size is known (the
    // network may be back), else checks again.
    this.failedStep = step === "file" ? (offer?.size != null ? "download" : "check") : step;
    const fromFile = (offer?.fromFile ?? false) && (step === "file" || FILE_CODES.has(code));
    const copy =
      fromFile && offer?.assetName ? { file: offer.assetName, version: offer.version } : undefined;
    this.stage = {
      step: "failed",
      failure: {
        code,
        message,
        ...installFailure(code, step, copy),
        retry: code !== "NOT_SUPPORTED",
        fromFile,
      },
    };
  }

  private finish(installed: boolean): void {
    this.generation++;
    this.open = false;
    this.offer = null;
    const resolve = this.resolvePending;
    this.pending = null;
    this.resolvePending = null;
    resolve?.(installed);
  }
}

export const duckdbInstallStore = new DuckdbInstallStore();
