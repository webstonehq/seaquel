import { load } from "@tauri-apps/plugin-store";

export type UserBackground = "none" | "datagrip" | "dbeaver";

interface PersistedOnboardingState {
	isFirstRun: boolean;
	userBackground: UserBackground;
	hasCompletedWizard: boolean;
	showWizardHints: boolean;
	dismissedHints: string[];
	learnEnabled: boolean;
}

const STORE_FILE = "onboarding_state.json";

class OnboardingStore {
	isFirstRun = $state(true);
	userBackground = $state<UserBackground>("none");
	hasCompletedWizard = $state(false);
	showWizardHints = $state(true);
	dismissedHints = $state<string[]>([]);
	learnEnabled = $state(true);

	private initialized = false;

	async initialize(): Promise<void> {
		if (this.initialized) return;

		try {
			const store = await load(STORE_FILE, {
				autoSave: false,
				defaults: { state: null },
			});

			const persisted = (await store.get("state")) as PersistedOnboardingState | null;

			if (persisted) {
				this.isFirstRun = persisted.isFirstRun;
				this.userBackground = persisted.userBackground;
				this.hasCompletedWizard = persisted.hasCompletedWizard;
				this.showWizardHints = persisted.showWizardHints;
				this.dismissedHints = persisted.dismissedHints || [];
				this.learnEnabled = persisted.learnEnabled ?? true;
			}

			this.initialized = true;
		} catch (error) {
			console.error("Failed to load onboarding state:", error);
			this.initialized = true;
		}
	}

	setBackground(background: UserBackground): void {
		this.userBackground = background;
		this.persist();
	}

	completeWizard(): void {
		this.hasCompletedWizard = true;
    this.isFirstRun = false;
		this.persist();
	}

	dismissHint(hintId: string): void {
		if (!this.dismissedHints.includes(hintId)) {
			this.dismissedHints = [...this.dismissedHints, hintId];
			this.persist();
		}
	}

	isHintDismissed(hintId: string): boolean {
		return this.dismissedHints.includes(hintId);
	}

	setShowWizardHints(show: boolean): void {
		this.showWizardHints = show;
		this.persist();
	}

	setLearnEnabled(enabled: boolean): void {
		this.learnEnabled = enabled;
		this.persist();
	}

	private async persist(): Promise<void> {
		try {
			const store = await load(STORE_FILE, {
				autoSave: true,
				defaults: { state: null },
			});

			const state: PersistedOnboardingState = {
				isFirstRun: this.isFirstRun,
				userBackground: this.userBackground,
				hasCompletedWizard: this.hasCompletedWizard,
				showWizardHints: this.showWizardHints,
				dismissedHints: this.dismissedHints,
				learnEnabled: this.learnEnabled,
			};

			await store.set("state", state);
			await store.save();
		} catch (error) {
			console.error("Failed to persist onboarding state:", error);
		}
	}
}

export const onboardingStore = new OnboardingStore();
