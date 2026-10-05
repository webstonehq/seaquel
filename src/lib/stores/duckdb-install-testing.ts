/**
 * A fake `DuckdbInstallService` for the page's tests (desktop DuckDB helper
 * plan, Task 5): each command is recorded, and the install and the file
 * install wait until the test finishes or fails them, as the Tauri commands
 * would. A second `install()` while one runs joins it as the real command
 * does (Task 4's `HelperInstalls`): the joiner gets the current progress at
 * once, then each update, and the same answer; cancel rejects every caller
 * with `CANCELLED`. No Tauri, no network, no file system.
 */
import { DuckdbHelperError } from "$lib/api/tauri";
import type {
  DuckdbHelperInstalled,
  DuckdbHelperOffer,
  DuckdbHelperProgress,
} from "$lib/api/tauri";
import type { DuckdbInstallService } from "./duckdb-install.svelte";

/** 11.5 MB, the helper's compressed size. */
export const HELPER_SIZE = 11_503_115;

export const ASSET_NAME = "seaquel-duckdb-aarch64-apple-darwin.gz";

export const MISSING: DuckdbHelperOffer = {
  status: "missing",
  version: "2026.10.1",
  size: HELPER_SIZE,
  sizeError: null,
  assetName: ASSET_NAME,
  fromFile: true,
};

interface Caller {
  onProgress?: (progress: DuckdbHelperProgress) => void;
  resolve: (installed: DuckdbHelperInstalled) => void;
  reject: (error: unknown) => void;
}

/** One running install (a download or a file), and everyone waiting on it. */
interface Running {
  callers: Caller[];
  latest: DuckdbHelperProgress | null;
}

export function helperError(code: string, message = `${code} happened`): DuckdbHelperError {
  return new DuckdbHelperError({ code, message });
}

export class FakeDuckdbInstall implements DuckdbInstallService {
  /** Every command, in order: `offer`, `install`, `cancel`, `pick`, `file <path>`. */
  readonly calls: string[] = [];
  /** Downloads actually started (a joined install isn't one). */
  downloads = 0;
  /** Per `install()` call, whether its caller listened to the progress. */
  readonly listened: boolean[] = [];
  /** What `offer` answers (an error is thrown). */
  offerAnswer: DuckdbHelperOffer | Error = MISSING;
  /** What the file picker answers. */
  picked: string | null = "/copies/seaquel-duckdb.gz";
  private running: Running | null = null;
  private waiters: Array<() => void> = [];

  async offer(): Promise<DuckdbHelperOffer> {
    this.calls.push("offer");
    if (this.offerAnswer instanceof Error) throw this.offerAnswer;
    return this.offerAnswer;
  }

  install(onProgress?: (progress: DuckdbHelperProgress) => void): Promise<DuckdbHelperInstalled> {
    this.calls.push("install");
    this.listened.push(onProgress !== undefined);
    if (this.running) return this.join(this.running, onProgress);
    this.downloads++;
    return this.start(onProgress);
  }

  async cancel(): Promise<boolean> {
    this.calls.push("cancel");
    const running = this.running;
    this.running = null;
    for (const caller of running?.callers ?? []) {
      caller.reject(helperError("CANCELLED", "the install was cancelled"));
    }
    return running !== null;
  }

  async pickFile(): Promise<string | null> {
    this.calls.push("pick");
    return this.picked;
  }

  installFromFile(path: string): Promise<DuckdbHelperInstalled> {
    this.calls.push(`file ${path}`);
    return this.start();
  }

  /** Whether an install (download or file) is waiting for the test. */
  get busy(): boolean {
    return this.running !== null;
  }

  /** Resolves once an install is running. */
  started(): Promise<void> {
    if (this.running) return Promise.resolve();
    return new Promise((resolve) => this.waiters.push(resolve));
  }

  progress(bytes: number, total = HELPER_SIZE): void {
    const running = this.running;
    if (!running) return;
    running.latest = { bytes, total };
    for (const caller of running.callers) caller.onProgress?.({ bytes, total });
  }

  finish(installed: DuckdbHelperInstalled = { downloaded: true, pruned: 0 }): void {
    for (const caller of this.take().callers) caller.resolve(installed);
  }

  fail(code: string, message?: string): void {
    for (const caller of this.take().callers) caller.reject(helperError(code, message));
  }

  private start(onProgress?: (progress: DuckdbHelperProgress) => void) {
    const running: Running = { callers: [], latest: null };
    this.running = running;
    const answer = this.join(running, onProgress);
    const waiters = this.waiters;
    this.waiters = [];
    for (const wake of waiters) wake();
    return answer;
  }

  private join(
    running: Running,
    onProgress?: (progress: DuckdbHelperProgress) => void,
  ): Promise<DuckdbHelperInstalled> {
    if (running.latest) onProgress?.(running.latest);
    return new Promise<DuckdbHelperInstalled>((resolve, reject) => {
      running.callers.push({ onProgress, resolve, reject });
    });
  }

  private take(): Running {
    const running = this.running;
    if (!running) throw new Error("no install is running");
    this.running = null;
    return running;
  }
}
