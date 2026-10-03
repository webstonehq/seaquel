/**
 * The demo's AI keys (phase 6 Task 8, Q2 B): the key a visitor enters in
 * Settings → AI, kept in this page's memory for the session and nowhere
 * else. Not in Core's metadata file (the provider is stored without it),
 * not in IndexedDB, `localStorage` or `sessionStorage`, never logged. A
 * reload, or closing the tab, forgets it. `CoreAi` reads it through
 * `aiKeyVault()` and sends it with each `ai` call, naming its provider, so
 * a module restarted after a trap needs nothing from here but the next call.
 *
 * Demo only: reached through `aiKeyVault()` and the AI settings store
 * behind `import.meta.env.VITE_BUILD_TARGET === "demo"`, so Rollup drops it
 * from the desktop and web builds.
 */
import type { AiKeyVault } from "./keyring";

export class SessionKeys implements AiKeyVault {
  private readonly keys = new Map<string, string>();

  set(providerId: string, key: string): void {
    if (key) this.keys.set(providerId, key);
    else this.keys.delete(providerId);
  }

  delete(providerId: string): void {
    this.keys.delete(providerId);
  }

  async hasAIApiKeyForProvider(providerId: string): Promise<boolean> {
    return this.keys.has(providerId);
  }

  async getAIApiKeyForProvider(providerId: string): Promise<string | null> {
    return this.keys.get(providerId) ?? null;
  }
}

let instance: SessionKeys | null = null;

/** This page's session keys (one per page load). */
export function sessionKeys(): SessionKeys {
  return (instance ??= new SessionKeys());
}
