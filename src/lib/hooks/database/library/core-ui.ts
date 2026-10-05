/**
 * `UiService` over Seaquel Core (desktop and web): the `ui` RPC group,
 * through the page's `RustStorageClient`, so its writes (a save, an
 * activation, and a first load, which copies a row) join the write queue
 * and land in the order the page issued them. The `pagehide` save goes as a
 * `keepalive` request outside the queue; `rev` orders it against the saves
 * still queued.
 */
import type { RustStorageClient } from "$lib/storage/rust-client";
import type { UiService, ViewState, ViewStateLoaded } from "./types";

/** What `CoreUi` needs of the storage client. */
export type UiCaller = Pick<RustStorageClient, "ui" | "saveWindowStateKeepalive">;

export class CoreUi implements UiService {
  /** `getCaller` is read per call, so the page's client can be swapped (tests). */
  constructor(private readonly getCaller: () => UiCaller) {}

  windowGet(windowId: string) {
    return this.getCaller().ui("windowGet", { windowId });
  }

  windowActivate(windowId: string, projectId: string) {
    return this.getCaller().ui("windowActivate", { windowId, projectId });
  }

  async windowStateLoad(windowId: string, projectId: string) {
    const { value, seq } = await this.getCaller().ui("windowStateLoad", { windowId, projectId });
    // Core stores the state as the page sent it; `null` when there was none.
    const loaded: ViewStateLoaded = {
      state: (value.state ?? null) as ViewState | null,
      rev: value.rev,
      copiedFrom: value.copiedFrom,
    };
    return { value: loaded, seq };
  }

  windowStateSave(windowId: string, projectId: string, rev: number, state: ViewState) {
    return this.getCaller().ui("windowStateSave", { windowId, projectId, rev, state });
  }

  windowStateSaveKeepalive(windowId: string, projectId: string, rev: number, state: ViewState) {
    return this.getCaller().saveWindowStateKeepalive({ windowId, projectId, rev, state });
  }
}
