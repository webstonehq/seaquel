import { getDatabase, appStateRepo } from "$lib/storage";
import { safeJsonParse } from "$lib/storage/create-repo";
import { licenseStore } from "./license.svelte.js";

/**
 * A one-time "are you using Seaquel for work?" nudge for unlicensed users.
 *
 * Seaquel's license is honor-system (free for personal use, paid for
 * commercial use), so this asks people to self-identify once they've clearly
 * gotten value, rather than gating anything. Usage is counted locally only.
 */

export type LicenseNudgeAnswer = "work" | "personal";

interface PersistedNudgeState {
  queryCount: number;
  activeDays: number;
  lastActiveDay: string | null;
  answer: LicenseNudgeAnswer | null;
  snoozedUntil: string | null;
}

const STORAGE_KEY = "license_nudge";
const QUERY_THRESHOLD = 100;
const ACTIVE_DAYS_THRESHOLD = 14;
const DAY_MS = 24 * 60 * 60 * 1000;
const SNOOZE_MS = 60 * DAY_MS;
/** How often to remind people who said they use Seaquel for work but haven't activated a license. */
const WORK_REMINDER_MS = 30 * DAY_MS;

class LicenseNudgeStore {
  queryCount = $state(0);
  activeDays = $state(0);
  lastActiveDay = $state<string | null>(null);
  answer = $state<LicenseNudgeAnswer | null>(null);
  snoozedUntil = $state<string | null>(null);

  private initialized = $state(false);

  /** Whether the nudge card should be visible right now. */
  get shouldShow(): boolean {
    if (!this.initialized || this.answer === "personal") return false;
    if (licenseStore.status !== "personal") return false;
    if (this.snoozedUntil && Date.now() < new Date(this.snoozedUntil).getTime()) return false;
    return this.queryCount >= QUERY_THRESHOLD || this.activeDays >= ACTIVE_DAYS_THRESHOLD;
  }

  /** True once the user has said they use Seaquel for work but is still unlicensed. */
  get isWorkReminder(): boolean {
    return this.answer === "work";
  }

  /** Which milestone to mention in the card. Queries win when both apply. */
  get milestone(): { kind: "queries"; count: number } | { kind: "days"; count: number } {
    return this.queryCount >= QUERY_THRESHOLD
      ? { kind: "queries", count: QUERY_THRESHOLD }
      : { kind: "days", count: ACTIVE_DAYS_THRESHOLD };
  }

  async initialize(): Promise<void> {
    if (this.initialized) return;

    try {
      const db = await getDatabase();
      const raw = await appStateRepo.get(db, STORAGE_KEY);
      const persisted = safeJsonParse<PersistedNudgeState | null>(raw, null);

      if (persisted) {
        this.queryCount = persisted.queryCount ?? 0;
        this.activeDays = persisted.activeDays ?? 0;
        this.lastActiveDay = persisted.lastActiveDay ?? null;
        this.answer = persisted.answer ?? null;
        this.snoozedUntil = persisted.snoozedUntil ?? null;
      }
    } catch (error) {
      console.error("Failed to load license nudge state:", error);
    } finally {
      this.initialized = true;
    }
  }

  /** Called whenever a query is executed. No-op until initialized (e.g. web builds). */
  recordQuery(): void {
    if (!this.initialized || this.answer) return;

    this.queryCount += 1;
    const today = localDay(new Date());
    if (this.lastActiveDay !== today) {
      this.lastActiveDay = today;
      this.activeDays += 1;
    }
    void this.persist();
  }

  respond(answer: LicenseNudgeAnswer): void {
    this.answer = answer;
    if (answer === "work") this.snoozeFor(WORK_REMINDER_MS);
    else void this.persist();
  }

  snooze(): void {
    this.snoozeFor(this.answer === "work" ? WORK_REMINDER_MS : SNOOZE_MS);
  }

  private snoozeFor(ms: number): void {
    this.snoozedUntil = new Date(Date.now() + ms).toISOString();
    void this.persist();
  }

  private async persist(): Promise<void> {
    try {
      const db = await getDatabase();
      const state: PersistedNudgeState = {
        queryCount: this.queryCount,
        activeDays: this.activeDays,
        lastActiveDay: this.lastActiveDay,
        answer: this.answer,
        snoozedUntil: this.snoozedUntil,
      };
      await appStateRepo.set(db, STORAGE_KEY, JSON.stringify(state));
    } catch (error) {
      console.error("Failed to persist license nudge state:", error);
    }
  }
}

function localDay(date: Date): string {
  const y = date.getFullYear();
  const m = String(date.getMonth() + 1).padStart(2, "0");
  const d = String(date.getDate()).padStart(2, "0");
  return `${y}-${m}-${d}`;
}

export const licenseNudgeStore = new LicenseNudgeStore();
