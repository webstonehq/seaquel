import { getSettings } from "$lib/hooks/database/library/index";
import { isTauri } from "$lib/utils/environment";
import { WriteOrder, onStoredChange, toastIfStorageFull } from "./settings-sync";

export type UserBackground = "none" | "datagrip" | "dbeaver";

interface PersistedOnboardingState {
  isFirstRun: boolean;
  userBackground: UserBackground;
  hasCompletedWizard: boolean;
  showWizardHints: boolean;
  dismissedHints: string[];
  learnEnabled: boolean;
}

/**
 * The onboarding state (desktop only). Phase 5d-2 (Decision 20): Core
 * reads it as the six defaults with the stored fields over them, and each
 * setter sends only the fields it changed (`onboardingPatch`), merged into
 * the stored record, so another window's change isn't lost.
 */
export class OnboardingStore {
  isFirstRun = $state(true);
  userBackground = $state<UserBackground>("none");
  hasCompletedWizard = $state(false);
  showWizardHints = $state(true);
  dismissedHints = $state<string[]>([]);
  learnEnabled = $state(true);

  /** True once the stored state was read. */
  private initialized = false;
  private loading: Promise<void> | null = null;
  /** Orders the patches' answers and reads (`seq`; no flicker back). */
  private readonly order = new WriteOrder("onboarding", () => this.read());

  constructor() {
    onStoredChange("onboarding", () => (this.initialized ? this.read() : undefined));
  }

  initialize(): Promise<void> {
    if (this.initialized) return Promise.resolve();
    this.loading = this.read().catch((error: unknown) => {
      // Stay uninitialized: a later `initialize` retries.
      console.error("Failed to load onboarding state:", error);
    });
    return this.loading;
  }

  private async read({ again = false } = {}): Promise<void> {
    const { value, seq } = await getSettings().getOnboarding();
    this.initialized = true;
    this.order.read(value, seq, (v) => this.show(v), { again });
  }

  private show(value: unknown): void {
    const s = (value ?? {}) as Partial<PersistedOnboardingState>;
    this.isFirstRun = s.isFirstRun ?? true;
    this.userBackground = s.userBackground ?? "none";
    this.hasCompletedWizard = s.hasCompletedWizard ?? false;
    this.showWizardHints = s.showWizardHints ?? true;
    this.dismissedHints = Array.isArray(s.dismissedHints) ? s.dismissedHints : [];
    this.learnEnabled = s.learnEnabled ?? true;
  }

  setBackground(background: UserBackground): void {
    this.userBackground = background;
    void this.persist({ userBackground: background });
  }

  completeWizard(): void {
    this.hasCompletedWizard = true;
    this.isFirstRun = false;
    void this.persist({ hasCompletedWizard: true, isFirstRun: false });
  }

  dismissHint(hintId: string): void {
    if (!this.dismissedHints.includes(hintId)) {
      this.dismissedHints = [...this.dismissedHints, hintId];
      void this.persist({ dismissedHints: this.dismissedHints });
    }
  }

  isHintDismissed(hintId: string): boolean {
    return this.dismissedHints.includes(hintId);
  }

  setShowWizardHints(show: boolean): void {
    this.showWizardHints = show;
    void this.persist({ showWizardHints: show });
  }

  setLearnEnabled(enabled: boolean): void {
    this.learnEnabled = enabled;
    void this.persist({ learnEnabled: enabled });
  }

  /**
   * Sends the changed fields. A load still out is waited for first; one
   * that failed doesn't stop it, since the patch only replaces its fields.
   */
  private async persist(patch: Partial<PersistedOnboardingState>): Promise<void> {
    // Onboarding is desktop-only: the web layout never loads it.
    if (!isTauri()) return;
    if (this.loading) await this.loading;
    try {
      await this.order.write(
        () => getSettings().patchOnboarding(patch),
        (v) => this.show(v),
      );
    } catch (error) {
      toastIfStorageFull(error);
      console.error("Failed to persist onboarding state:", error);
      // What shows goes back to what's stored, as the theme store does.
      await this.read({ again: true }).catch(() => {});
    }
  }
}

export const onboardingStore = new OnboardingStore();
