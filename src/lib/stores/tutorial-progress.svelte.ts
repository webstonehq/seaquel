import { getStorage } from "$lib/storage";
import { LESSONS } from "$lib/tutorial/lessons";
import type { SerializableQueryBuilderState } from "$lib/hooks/query-builder.svelte";

class TutorialProgressStore {
  /** Map of lessonId -> Set of completed challenge IDs */
  completedChallenges = $state<Record<string, Set<string>>>({});

  /** Map of lessonId -> challengeId -> saved query state */
  challengeStates = $state<Record<string, Record<string, SerializableQueryBuilderState>>>({});

  /** Whether the store has been initialized (loaded from persistence) */
  isInitialized = $state(false);

  async initialize(): Promise<void> {
    if (this.isInitialized) return;

    try {
      const rows = await getStorage().tutorial.loadAll();

      const challenges: Record<string, Set<string>> = {};
      const states: Record<string, Record<string, SerializableQueryBuilderState>> = {};

      for (const row of rows) {
        // Build completed challenges sets
        if (!challenges[row.lessonId]) {
          challenges[row.lessonId] = new Set();
        }
        challenges[row.lessonId].add(row.challengeId);

        // Build challenge states
        if (row.state) {
          if (!states[row.lessonId]) {
            states[row.lessonId] = {};
          }
          states[row.lessonId][row.challengeId] = JSON.parse(row.state);
        }
      }

      this.completedChallenges = challenges;
      this.challengeStates = states;
      this.isInitialized = true;
    } catch (error) {
      console.error("Failed to load tutorial progress:", error);
      this.isInitialized = true;
    }
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
        await getStorage().tutorial.removeLesson(lessonId);
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
      await getStorage().tutorial.removeAll();
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
      await getStorage().tutorial.save(lessonId, challengeId, state);
    } catch (error) {
      console.error("Failed to persist tutorial progress:", error);
    }
  }
}

export const tutorialProgressStore = new TutorialProgressStore();
