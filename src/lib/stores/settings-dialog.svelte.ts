export type SettingsSection = "app-info" | "theme" | "themes";
export type SettingsGroup = "general" | "appearance";
export type SettingsView = SettingsGroup | SettingsSection;

// Map sections to their parent groups
export const sectionToGroup: Record<SettingsSection, SettingsGroup> = {
	"app-info": "general",
	"theme": "appearance",
	"themes": "appearance",
};

// Map groups to their sections
export const groupSections: Record<SettingsGroup, SettingsSection[]> = {
	"general": ["app-info"],
	"appearance": ["theme", "themes"],
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
		return this.activeView === "general" || this.activeView === "appearance";
	}

	// Get the active group (either directly selected or parent of selected section)
	getActiveGroup(): SettingsGroup {
		if (this.activeView === "general" || this.activeView === "appearance") {
			return this.activeView;
		}
		return sectionToGroup[this.activeView as SettingsSection];
	}
}

export const settingsDialogStore = new SettingsDialogStore();
