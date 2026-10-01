/**
 * Decision 12a's one-time notice. When Core first opens a file, it moves
 * the passwords the driver reads out of saved connection strings into the
 * keychain (desktop) and strips every secret from the strings. The
 * connections whose secrets couldn't all be moved (on web: every stripped
 * one) are listed in the `app_state` key `connectionStringSecretsNotice`, a
 * JSON id list. After the page has loaded its connections it reads the
 * list, names them once, and clears the key when the user dismisses it.
 */
import { getSettings } from "$lib/hooks/database/library/index";
import { log } from "$lib/utils/logger";

/** Core's `app_state` key for the notice (`seaquel-core`'s upgrade). */
export const CONNECTION_SECRETS_NOTICE_KEY = "connectionStringSecretsNotice" as const;

/** The ids in the stored list, or `[]` when it's missing or not a list of strings. */
export function parseNoticeIds(value: string | null): string[] {
  if (!value) return [];
  try {
    const parsed: unknown = JSON.parse(value);
    return Array.isArray(parsed) ? parsed.filter((id): id is string => typeof id === "string") : [];
  } catch {
    return [];
  }
}

class ConnectionSecretsNotice {
  /** The connections to name, while the notice is open. */
  names = $state<string[]>([]);
  open = $state(false);

  /**
   * Read the list and open the notice if it names a connection. `nameOf`
   * gives a listed connection's name; one the page doesn't have (removed
   * since) is left out.
   */
  async check(nameOf: (id: string) => string | undefined): Promise<void> {
    let ids: string[];
    try {
      ids = parseNoticeIds((await getSettings().getSetting(CONNECTION_SECRETS_NOTICE_KEY)).value);
    } catch (error) {
      void log.warn("Reading the connection secrets notice failed:", error);
      return;
    }
    if (ids.length === 0) return;
    const names = ids.map(nameOf).filter((n): n is string => n !== undefined);
    if (names.length === 0) {
      // Every listed connection is gone: nothing to say.
      await this.clear();
      return;
    }
    this.names = names;
    this.open = true;
  }

  /** The user has read it: close it and clear the stored list, so it shows once. */
  async dismiss(): Promise<void> {
    this.open = false;
    this.names = [];
    await this.clear();
  }

  private async clear(): Promise<void> {
    try {
      await getSettings().setSetting(CONNECTION_SECRETS_NOTICE_KEY, null);
    } catch (error) {
      void log.warn("Clearing the connection secrets notice failed:", error);
    }
  }
}

export const connectionSecretsNotice = new ConnectionSecretsNotice();
