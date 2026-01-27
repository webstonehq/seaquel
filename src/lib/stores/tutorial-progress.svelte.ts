import { load } from "@tauri-apps/plugin-store";
import { isTauri } from "$lib/utils/environment";
import { LESSONS } from "$lib/tutorial/lessons";
import type { SerializableQueryBuilderState } from "$lib/hooks/query-builder.svelte";

interface PersistedTutorialProgress {
	/** Map of lessonId -> Set of completed challenge IDs */
	completedChallenges: Record<string, string[]>;
	/** Map of lessonId -> challengeId -> saved query state */
	challengeStates: Record<string, Record<string, SerializableQueryBuilderState>>;
}

const STORE_FILE = "tutorial_progress.json";
const LOCAL_STORAGE_KEY = "seaquel_tutorial_progress";

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
			if (isTauri()) {
				const store = await load(STORE_FILE, {
					autoSave: false,
					defaults: { state: null },
				});

				const persisted = (await store.get("state")) as PersistedTutorialProgress | null;

				if (persisted?.completedChallenges) {
					// Convert arrays back to Sets
					const challenges: Record<string, Set<string>> = {};
					for (const [lessonId, challengeIds] of Object.entries(persisted.completedChallenges)) {
						challenges[lessonId] = new Set(challengeIds);
					}
					this.completedChallenges = challenges;
				}
				if (persisted?.challengeStates) {
					this.challengeStates = persisted.challengeStates;
				}
			} else {
				// Browser/demo mode - use localStorage
				const stored = localStorage.getItem(LOCAL_STORAGE_KEY);
				if (stored) {
					const persisted = JSON.parse(stored) as PersistedTutorialProgress;
					if (persisted?.completedChallenges) {
						const challenges: Record<string, Set<string>> = {};
						for (const [lessonId, challengeIds] of Object.entries(persisted.completedChallenges)) {
							challenges[lessonId] = new Set(challengeIds);
						}
						this.completedChallenges = challenges;
					}
					if (persisted?.challengeStates) {
						this.challengeStates = persisted.challengeStates;
					}
				}
			}

			this.isInitialized = true;
		} catch (error) {
			console.error("Failed to load tutorial progress:", error);
			this.isInitialized = true;
		}
	}

	/**
	 * Mark a challenge as completed for a lesson, optionally saving its state
	 */
	completeChallenge(lessonId: string, challengeId: string, state?: SerializableQueryBuilderState): void {
		if (!this.completedChallenges[lessonId]) {
			this.completedChallenges[lessonId] = new Set();
		}
		this.completedChallenges[lessonId].add(challengeId);
		// Trigger reactivity
		this.completedChallenges = { ...this.completedChallenges };

		// Save state if provided
		if (state) {
			this.saveChallengeState(lessonId, challengeId, state);
		} else {
			this.persist();
		}
	}

	/**
	 * Save the query builder state for a challenge
	 */
	saveChallengeState(lessonId: string, challengeId: string, state: SerializableQueryBuilderState): void {
		if (!this.challengeStates[lessonId]) {
			this.challengeStates[lessonId] = {};
		}
		this.challengeStates[lessonId][challengeId] = state;
		// Trigger reactivity
		this.challengeStates = { ...this.challengeStates };
		this.persist();
	}

	/**
	 * Get the saved state for a challenge, if any
	 */
	getChallengeState(lessonId: string, challengeId: string): SerializableQueryBuilderState | undefined {
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
	resetLesson(lessonId: string): void {
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
			this.persist();
		}
	}

	/**
	 * Reset all tutorial progress
	 */
	resetAll(): void {
		this.completedChallenges = {};
		this.challengeStates = {};
		this.persist();
	}

	private async persist(): Promise<void> {
		try {
			// Convert Sets to arrays for serialization
			const serialized: Record<string, string[]> = {};
			for (const [lessonId, challengeSet] of Object.entries(this.completedChallenges)) {
				serialized[lessonId] = Array.from(challengeSet);
			}

			const state: PersistedTutorialProgress = {
				completedChallenges: serialized,
				challengeStates: this.challengeStates,
			};

			if (isTauri()) {
				const store = await load(STORE_FILE, {
					autoSave: true,
					defaults: { state: null },
				});

				await store.set("state", state);
				await store.save();
			} else {
				// Browser/demo mode - use localStorage
				localStorage.setItem(LOCAL_STORAGE_KEY, JSON.stringify(state));
			}
		} catch (error) {
			console.error("Failed to persist tutorial progress:", error);
		}
	}
}

export const tutorialProgressStore = new TutorialProgressStore();
