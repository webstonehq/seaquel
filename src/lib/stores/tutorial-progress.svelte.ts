import { getSettings } from "$lib/hooks/database/library/index";
import type { TutorialProgress } from "$lib/hooks/database/library/types";
import { LESSONS } from "$lib/tutorial/lessons";
import { log } from "$lib/utils/logger";
import type { SerializableQueryBuilderState } from "$lib/hooks/query-builder.svelte";
import { WriteOrder, onStoredChange, toastIfStorageFull } from "./settings-sync";

/**
 * Tutorial progress: one row per challenge, written by `settings` calls
 * (phase 5d-2) that answer every row, applied by the `seq` rule; another
 * window's progress is read again at once.
 */
export class TutorialProgressStore {
  /** Map of lessonId -> Set of completed challenge IDs */
  completedChallenges = $state<Record<string, Set<string>>>({});

  /** Map of lessonId -> challengeId -> saved query state */
  challengeStates = $state<Record<string, Record<string, SerializableQueryBuilderState>>>({});

  /** Whether the store has been initialized (loaded from persistence) */
  isInitialized = $state(false);
  /** Orders the writes' answers and other windows' reads (`seq`, no flicker back). */
  private readonly order = new WriteOrder("tutorial", () => this.reload());

  constructor() {
    onStoredChange("tutorial", () => (this.isInitialized ? this.reload() : undefined));
  }

  async initialize(): Promise<void> {
    if (this.isInitialized) return;

    try {
      const { value, seq } = await getSettings().listTutorial();
      this.order.read(value, seq, (rows) => this.show(rows));
      this.isInitialized = true;
    } catch (error) {
      console.error("Failed to load tutorial progress:", error);
      this.isInitialized = true;
    }
  }

  /** Another window's progress: read every row again. */
  async reload(): Promise<void> {
    const { value, seq } = await getSettings().listTutorial();
    this.order.read(value, seq, (rows) => this.show(rows));
  }

  /** Shows the stored rows. */
  private show(rows: TutorialProgress[]): void {
    const challenges: Record<string, Set<string>> = {};
    const states: Record<string, Record<string, SerializableQueryBuilderState>> = {};

    for (const row of rows) {
      // Build completed challenges sets
      if (!challenges[row.lessonId]) {
        challenges[row.lessonId] = new Set();
      }
      challenges[row.lessonId].add(row.challengeId);

      // Build challenge states. A state that doesn't parse is skipped,
      // not allowed to empty everyone else's progress.
      if (row.state) {
        let parsed: SerializableQueryBuilderState;
        try {
          parsed = JSON.parse(row.state);
        } catch (error) {
          void log.warn(
            `Skipping the saved state of tutorial challenge ${row.lessonId}/${row.challengeId}:`,
            error,
          );
          continue;
        }
        if (!states[row.lessonId]) {
          states[row.lessonId] = {};
        }
        states[row.lessonId][row.challengeId] = parsed;
      }
    }

    this.completedChallenges = challenges;
    this.challengeStates = states;
  }

  /**
   * Mark a challenge as completed for a lesson, optionally saving its state
   */
  async completeChallenge(
    lessonId: string,
    challengeId: string,
    state?: SerializableQueryBuilderState,
  ): Promise<void> {
    if (!this.completedChallenges[lessonId]) {
      this.completedChallenges[lessonId] = new Set();
    }
    this.completedChallenges[lessonId].add(challengeId);
    // Trigger reactivity
    this.completedChallenges = { ...this.completedChallenges };

    // Save state if provided
    if (state) {
      await this.saveChallengeState(lessonId, challengeId, state);
    } else {
      await this.persistSingle(lessonId, challengeId, null);
    }
  }

  /**
   * Save the query builder state for a challenge
   */
  async saveChallengeState(
    lessonId: string,
    challengeId: string,
    state: SerializableQueryBuilderState,
  ): Promise<void> {
    if (!this.challengeStates[lessonId]) {
      this.challengeStates[lessonId] = {};
    }
    this.challengeStates[lessonId][challengeId] = state;
    // Trigger reactivity
    this.challengeStates = { ...this.challengeStates };
    await this.persistSingle(lessonId, challengeId, JSON.stringify(state));
  }

  /**
   * Get the saved state for a challenge, if any
   */
  getChallengeState(
    lessonId: string,
    challengeId: string,
  ): SerializableQueryBuilderState | undefined {
    return this.challengeStates[lessonId]?.[challengeId];
  }

  /**
   * Check if a challenge is completed
   */
  isChallengeCompleted(lessonId: string, challengeId: string): boolean {
    return this.completedChallenges[lessonId]?.has(challengeId) ?? false;
  }

  /**
   * Get the number of completed challenges for a lesson
   */
  getCompletedCount(lessonId: string): number {
    return this.completedChallenges[lessonId]?.size ?? 0;
  }

  /**
   * Get the total number of challenges for a lesson
   */
  getTotalChallenges(lessonId: string): number {
    const lesson = LESSONS[lessonId];
    return lesson?.challenges.length ?? 0;
  }

  /**
   * Check if a lesson is fully completed
   */
  isLessonCompleted(lessonId: string): boolean {
    const total = this.getTotalChallenges(lessonId);
    const completed = this.getCompletedCount(lessonId);
    return total > 0 && completed >= total;
  }

  /**
   * Get the set of completed challenge IDs for a lesson
   */
  getCompletedChallenges(lessonId: string): Set<string> {
    return this.completedChallenges[lessonId] ?? new Set();
  }

  /**
   * Reset progress for a specific lesson
   */
  async resetLesson(lessonId: string): Promise<void> {
    let changed = false;
    if (this.completedChallenges[lessonId]) {
      delete this.completedChallenges[lessonId];
      this.completedChallenges = { ...this.completedChallenges };
      changed = true;
    }
    if (this.challengeStates[lessonId]) {
      delete this.challengeStates[lessonId];
      this.challengeStates = { ...this.challengeStates };
      changed = true;
    }
    if (changed) {
      try {
        await this.order.write(() => getSettings().removeTutorialLesson(lessonId));
      } catch (error) {
        console.error("Failed to reset tutorial lesson:", error);
      }
    }
  }

  /**
   * Reset all tutorial progress
   */
  async resetAll(): Promise<void> {
    this.completedChallenges = {};
    this.challengeStates = {};
    try {
      await this.order.write(() => getSettings().resetTutorial());
    } catch (error) {
      console.error("Failed to reset all tutorial progress:", error);
    }
  }

  private async persistSingle(
    lessonId: string,
    challengeId: string,
    state: string | null,
  ): Promise<void> {
    try {
      // What it changed is shown already; another window's progress read
      // while it was on its way is read again once it's done.
      await this.order.write(() => getSettings().saveTutorial(lessonId, challengeId, state));
    } catch (error) {
      toastIfStorageFull(error);
      console.error("Failed to persist tutorial progress:", error);
    }
  }
}

export const tutorialProgressStore = new TutorialProgressStore();
