export type SettingsSection =
  | "app-info"
  | "license"
  | "team"
  | "airgap"
  | "theme"
  | "themes"
  | "editor"
  | "ai-feature"
  | "learn"
  | "ai-provider"
  | "ai-privacy"
  | "query-history"
  | "pending-changes";
export type SettingsGroup = "general" | "appearance" | "features" | "ai";
export type SettingsView = SettingsGroup | SettingsSection;

// Map sections to their parent groups
export const sectionToGroup: Record<SettingsSection, SettingsGroup> = {
  "app-info": "general",
  license: "general",
  team: "general",
  airgap: "general",
  theme: "appearance",
  themes: "appearance",
  editor: "appearance",
  "ai-feature": "features",
  learn: "features",
  "ai-provider": "ai",
  "ai-privacy": "ai",
  "query-history": "general",
  "pending-changes": "features",
};

// Map groups to their sections
export const groupSections: Record<SettingsGroup, SettingsSection[]> = {
  general: ["app-info", "license", "team", "airgap", "query-history"],
  appearance: ["theme", "themes", "editor"],
  features: ["ai-feature", "learn", "pending-changes"],
  ai: ["ai-provider", "ai-privacy"],
};

class SettingsDialogStore {
  isOpen = $state(false);
  activeView = $state<SettingsView>("general");

  open(view?: SettingsView) {
    this.activeView = view ?? "general";
    this.isOpen = true;
  }

  close() {
    this.isOpen = false;
  }

  setView(view: SettingsView) {
    this.activeView = view;
  }

  // Check if we're viewing a group (showing all sections)
  isGroupView(): boolean {
    return (
      this.activeView === "general" ||
      this.activeView === "appearance" ||
      this.activeView === "features" ||
      this.activeView === "ai"
    );
  }

  // Get the active group (either directly selected or parent of selected section)
  getActiveGroup(): SettingsGroup {
    if (
      this.activeView === "general" ||
      this.activeView === "appearance" ||
      this.activeView === "features" ||
      this.activeView === "ai"
    ) {
      return this.activeView;
    }
    return sectionToGroup[this.activeView as SettingsSection];
  }
}

export const settingsDialogStore = new SettingsDialogStore();
