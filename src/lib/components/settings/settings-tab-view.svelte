<script lang="ts">
	import { m } from "$lib/paraglide/messages.js";
	import * as Breadcrumb from "$lib/components/ui/breadcrumb/index.js";
	import * as Sidebar from "$lib/components/ui/sidebar/index.js";
	import {
		type SettingsSection,
		type SettingsGroup,
		type SettingsView,
		groupSections,
		sectionToGroup,
	} from "$lib/stores/settings-dialog.svelte.js";
	import SettingsIcon from "@lucide/svelte/icons/settings";
	import PaletteIcon from "@lucide/svelte/icons/palette";
	import InfoIcon from "@lucide/svelte/icons/info";
	import SunMoonIcon from "@lucide/svelte/icons/sun-moon";
	import SwatchBookIcon from "@lucide/svelte/icons/swatch-book";
	import BlocksIcon from "@lucide/svelte/icons/blocks";
	import KeyIcon from "@lucide/svelte/icons/key-round";
	import SparklesIcon from "@lucide/svelte/icons/sparkles";
	import ShieldIcon from "@lucide/svelte/icons/shield";
	import HistoryIcon from "@lucide/svelte/icons/history";
	import KeyboardIcon from "@lucide/svelte/icons/keyboard";
	import WifiOffIcon from "@lucide/svelte/icons/wifi-off";
	import type { SettingsTab } from "$lib/types";
	import { aiSettingsStore } from "$lib/stores/ai-settings.svelte.js";
	import { isTauri, isWeb } from "$lib/utils/environment";
	import UsersIcon from "@lucide/svelte/icons/users";
	import { useDatabase } from "$lib/hooks/database.svelte.js";

	import AppInfoSection from "./general/app-info-section.svelte";
	import LicenseSection from "./general/license-section.svelte";
	import QueryHistorySection from "./general/query-history-section.svelte";
	import ThemeSection from "./appearance/theme-section.svelte";
	import ThemesSection from "./appearance/themes-section.svelte";
	import EditorSection from "./appearance/editor-section.svelte";
	import FeaturesSection from "./features/features-section.svelte";
	import AiProviderSection from "./ai/ai-provider-section.svelte";
	import AiPrivacySection from "./ai/ai-privacy-section.svelte";
	import TeamSection from "./general/team-section.svelte";
	import AirgapSection from "./general/airgap-section.svelte";

	interface Props {
		tab: SettingsTab;
	}

	let { tab }: Props = $props();

	const db = useDatabase();

	// Local navigation state derived from tab's activeView
	const activeView = $derived((tab.activeView ?? "all") as SettingsView | "all");

	function setView(view: SettingsView | "all") {
		db.settingsTabs.setSettingsView(tab.id, view);
	}

	function isGroupView(): boolean {
		return (
			activeView === "all" ||
			activeView === "general" ||
			activeView === "appearance" ||
			activeView === "features" ||
			activeView === "ai"
		);
	}

	function getActiveGroup(): SettingsGroup | "all" {
		if (activeView === "all") return "all";
		if (
			activeView === "general" ||
			activeView === "appearance" ||
			activeView === "features" ||
			activeView === "ai"
		) {
			return activeView;
		}
		return sectionToGroup[activeView as SettingsSection];
	}

	// Navigation structure
	type NavSubItem = {
		id: SettingsSection;
		name: string;
		icon: typeof InfoIcon;
	};

	type NavGroup = {
		id: SettingsGroup;
		name: string;
		icon: typeof SettingsIcon;
		items: NavSubItem[];
	};

	const navGroups: NavGroup[] = $derived([
		{
			id: "general",
			name: m.settings_general(),
			icon: SettingsIcon,
			items: [
				{ id: "app-info", name: m.settings_app_info(), icon: InfoIcon },
				...(isTauri() ? [{ id: "license" as const, name: m.settings_license(), icon: KeyIcon }] : []),
				...(isWeb() ? [{ id: "team" as const, name: "Team", icon: UsersIcon }] : []),
				...(isWeb() ? [{ id: "airgap" as const, name: "Offline bundle", icon: WifiOffIcon }] : []),
				{ id: "query-history", name: m.settings_query_history(), icon: HistoryIcon },
			],
		},
		{
			id: "appearance",
			name: m.settings_appearance(),
			icon: PaletteIcon,
			items: [
				{ id: "theme", name: m.settings_theme(), icon: SunMoonIcon },
				{ id: "themes", name: m.settings_themes(), icon: SwatchBookIcon },
				{ id: "editor", name: m.settings_editor(), icon: KeyboardIcon },
			],
		},
		{
			id: "features",
			name: m.settings_features(),
			icon: BlocksIcon,
			items: [],
		},
		{
			id: "ai",
			name: m.settings_ai(),
			icon: SparklesIcon,
			items: [
				{ id: "ai-provider", name: m.settings_ai_provider(), icon: SparklesIcon },
				{ id: "ai-privacy", name: m.settings_ai_privacy(), icon: ShieldIcon },
			],
		},
	]);

	// Find the active group for breadcrumb display
	const activeGroupObj = $derived(() => {
		const groupId = getActiveGroup();
		return navGroups.find((g) => g.id === groupId);
	});

	// Find the active section name (only when viewing a specific section)
	const activeSectionName = $derived(() => {
		if (isGroupView()) return null;
		for (const group of navGroups) {
			const item = group.items.find((item) => item.id === activeView);
			if (item) return item.name;
		}
		return null;
	});

	// Check if a section should be shown
	function shouldShowSection(sectionId: SettingsSection): boolean {
		if (activeView === "all") return true;
		if (activeView === sectionId) return true;
		if (activeView === "general" || activeView === "appearance" || activeView === "features" || activeView === "ai") {
			return groupSections[activeView].includes(sectionId);
		}
		return false;
	}

	function lookupGroup(id: string): SettingsGroup | undefined {
		return sectionToGroup[id as SettingsSection];
	}

	// Get scroll-spy section only if it belongs to the current view's group
	function visibleInGroup(groupId: SettingsGroup): string | null {
		return visibleSectionId && lookupGroup(visibleSectionId) === groupId ? visibleSectionId : null;
	}

	// Check if a menu item is active
	function isItemActive(itemId: SettingsSection): boolean {
		if (activeView === "all") return visibleSectionId === itemId;
		if (activeView === itemId) return true;
		if (activeView === "general") return (visibleInGroup("general") ?? "app-info") === itemId;
		if (activeView === "appearance") return (visibleInGroup("appearance") ?? "theme") === itemId;
		if (activeView === "features") return false;
		if (activeView === "ai") return (visibleInGroup("ai") ?? "ai-provider") === itemId;
		return false;
	}

	// Check if a group label is active (for scroll spy highlighting)
	function isGroupActive(groupId: SettingsGroup): boolean {
		if (activeView === "all" && visibleSectionId) return lookupGroup(visibleSectionId) === groupId;
		if (activeView === groupId) return true;
		if (!isGroupView() && activeView) return lookupGroup(activeView) === groupId;
		return false;
	}

	// Scroll spy for highlighting visible section in sidebar nav
	let visibleSectionId = $state<string | null>(null);
</script>

<Sidebar.Provider style="min-height: 0;" class="h-full">
	<Sidebar.Root collapsible="none" class="hidden md:flex border-r">
		<Sidebar.Content>
			{#each navGroups as group (group.id)}
			{#if group.id !== "ai" || aiSettingsStore.settings.enabled}
				<Sidebar.Group>
					<Sidebar.GroupLabel
						class="gap-2 cursor-pointer hover:text-foreground transition-colors {isGroupActive(group.id) ? 'text-foreground' : ''}"
						onclick={() => setView(group.id)}
					>
						<group.icon class="size-4" />
						<span>{group.name}</span>
					</Sidebar.GroupLabel>
					<Sidebar.GroupContent>
						<Sidebar.Menu>
							{#each group.items as item (item.id)}
								<Sidebar.MenuItem>
									<Sidebar.MenuButton
										isActive={isItemActive(item.id)}
										onclick={() => setView(item.id)}
									>
										<item.icon class="size-4" />
										<span>{item.name}</span>
									</Sidebar.MenuButton>
								</Sidebar.MenuItem>
							{/each}
						</Sidebar.Menu>
					</Sidebar.GroupContent>
				</Sidebar.Group>
			{/if}
			{/each}
		</Sidebar.Content>
	</Sidebar.Root>
	<main class="flex flex-1 flex-col overflow-hidden">
		<header
			class="flex h-12 shrink-0 items-center gap-2 border-b"
		>
			<div class="flex items-center gap-2 px-4">
				<Breadcrumb.Root>
					<Breadcrumb.List>
						<Breadcrumb.Item class="hidden md:block">
							{#if activeView === "all"}
								<Breadcrumb.Page>{m.settings_title()}</Breadcrumb.Page>
							{:else}
								<Breadcrumb.Link href="#" onclick={() => setView("all")}>
									{m.settings_title()}
								</Breadcrumb.Link>
							{/if}
						</Breadcrumb.Item>
						{#if activeView !== "all"}
							<Breadcrumb.Separator class="hidden md:block" />
							<Breadcrumb.Item class="hidden md:block">
								{#if activeSectionName()}
									<Breadcrumb.Link
										href="#"
										onclick={() => setView(activeGroupObj()?.id ?? "general")}
									>
										{activeGroupObj()?.name}
									</Breadcrumb.Link>
								{:else}
									<Breadcrumb.Page>{activeGroupObj()?.name}</Breadcrumb.Page>
								{/if}
							</Breadcrumb.Item>
							{#if activeSectionName()}
								<Breadcrumb.Separator class="hidden md:block" />
								<Breadcrumb.Item>
									<Breadcrumb.Page>{activeSectionName()}</Breadcrumb.Page>
								</Breadcrumb.Item>
							{/if}
						{/if}
					</Breadcrumb.List>
				</Breadcrumb.Root>
			</div>
		</header>
		<div class="flex flex-1 flex-col gap-6 overflow-y-auto p-4 pt-4" {@attach (container) => {
			const _view = activeView;
			const sectionEls = container.querySelectorAll<HTMLElement>("[data-section]");
			if (sectionEls.length === 0) return;
			const observer = new IntersectionObserver(
				(entries) => {
					let topSection: { id: string; top: number } | null = null;
					for (const entry of entries) {
						if (entry.isIntersecting) {
							const rect = entry.boundingClientRect;
							if (!topSection || rect.top < topSection.top) {
								topSection = { id: entry.target.getAttribute("data-section")!, top: rect.top };
							}
						}
					}
					if (topSection) {
						visibleSectionId = topSection.id;
					}
				},
				{ root: container, rootMargin: "0px 0px -60% 0px", threshold: 0 },
			);
			for (const el of sectionEls) observer.observe(el);
			return () => observer.disconnect();
		}}>
			{#if shouldShowSection("app-info")}
				<AppInfoSection {tab} />
			{/if}

			{#if shouldShowSection("license")}
				<LicenseSection />
			{/if}

			{#if shouldShowSection("team")}
				<TeamSection />
			{/if}

			{#if isWeb() && shouldShowSection("airgap")}
				<AirgapSection />
			{/if}

			{#if shouldShowSection("query-history")}
				<QueryHistorySection />
			{/if}

			{#if shouldShowSection("theme")}
				<ThemeSection />
			{/if}

			{#if shouldShowSection("themes")}
				<ThemesSection />
			{/if}

			{#if shouldShowSection("editor")}
				<EditorSection />
			{/if}

			{#if shouldShowSection("ai-feature") || shouldShowSection("learn") || shouldShowSection("pending-changes")}
				<FeaturesSection />
			{/if}

			{#if shouldShowSection("ai-provider")}
				<AiProviderSection />
			{/if}

			{#if shouldShowSection("ai-privacy")}
				<AiPrivacySection />
			{/if}
		</div>
	</main>
</Sidebar.Provider>
