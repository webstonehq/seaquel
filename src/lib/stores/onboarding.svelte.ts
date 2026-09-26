import { getStorage } from "$lib/storage";
import { skipUnloadedSave } from "$lib/storage/load-guard";

export type UserBackground = "none" | "datagrip" | "dbeaver";

interface PersistedOnboardingState {
  isFirstRun: boolean;
  userBackground: UserBackground;
  hasCompletedWizard: boolean;
  showWizardHints: boolean;
  dismissedHints: string[];
  learnEnabled: boolean;
}

class OnboardingStore {
  isFirstRun = $state(true);
  userBackground = $state<UserBackground>("none");
  hasCompletedWizard = $state(false);
  showWizardHints = $state(true);
  dismissedHints = $state<string[]>([]);
  learnEnabled = $state(true);

  /**
   * True once the stored state was read. Saving writes the whole record, so
   * until then a save would reset the wizard, hints and Learn setting.
   */
  private initialized = false;

  async initialize(): Promise<void> {
    if (this.initialized) return;

    try {
      const persisted = (await getStorage().onboarding.load()) as PersistedOnboardingState | null;

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
      // Stay uninitialized: a later `initialize` retries, and saves are off.
      console.error("Failed to load onboarding state:", error);
    }
  }

  setBackground(background: UserBackground): void {
    this.userBackground = background;
    void this.persist();
  }

  completeWizard(): void {
    this.hasCompletedWizard = true;
    this.isFirstRun = false;
    void this.persist();
  }

  dismissHint(hintId: string): void {
    if (!this.dismissedHints.includes(hintId)) {
      this.dismissedHints = [...this.dismissedHints, hintId];
      void this.persist();
    }
  }

  isHintDismissed(hintId: string): boolean {
    return this.dismissedHints.includes(hintId);
  }

  setShowWizardHints(show: boolean): void {
    this.showWizardHints = show;
    void this.persist();
  }

  setLearnEnabled(enabled: boolean): void {
    this.learnEnabled = enabled;
    void this.persist();
  }

  private async persist(): Promise<void> {
    if (!this.initialized) {
      skipUnloadedSave("onboarding state");
      return;
    }
    try {
      const state: PersistedOnboardingState = {
        isFirstRun: this.isFirstRun,
        userBackground: this.userBackground,
        hasCompletedWizard: this.hasCompletedWizard,
        showWizardHints: this.showWizardHints,
        dismissedHints: this.dismissedHints,
        learnEnabled: this.learnEnabled,
      };
      await getStorage().onboarding.save(state);
    } catch (error) {
      console.error("Failed to persist onboarding state:", error);
    }
  }
}

export const onboardingStore = new OnboardingStore();
