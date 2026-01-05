<script lang="ts">
	import { m } from "$lib/paraglide/messages.js";
	import * as Breadcrumb from "$lib/components/ui/breadcrumb/index.js";
	import * as Dialog from "$lib/components/ui/dialog/index.js";
	import * as Sidebar from "$lib/components/ui/sidebar/index.js";
	import {
		Select,
		SelectContent,
		SelectItem,
		SelectTrigger,
	} from "$lib/components/ui/select";
	import {
		settingsDialogStore,
		type SettingsSection,
		type SettingsGroup,
		type SettingsView,
		groupSections,
	} from "$lib/stores/settings-dialog.svelte.js";
	import { getVersion } from "@tauri-apps/api/app";
	import { appConfigDir, appDataDir } from "@tauri-apps/api/path";
	import { setMode, resetMode, mode } from "mode-watcher";
	import { setTheme } from "@tauri-apps/api/app";
	import SettingsIcon from "@lucide/svelte/icons/settings";
	import PaletteIcon from "@lucide/svelte/icons/palette";
	import InfoIcon from "@lucide/svelte/icons/info";
	import SunMoonIcon from "@lucide/svelte/icons/sun-moon";

	// App info state
	let appVersion = $state<string>("");
	let configPath = $state<string>("");
	let dataPath = $state<string>("");

	// Load app info when dialog opens
	$effect(() => {
		if (settingsDialogStore.isOpen) {
			loadAppInfo();
		}
	});

	async function loadAppInfo() {
		try {
			appVersion = await getVersion();
			configPath = await appConfigDir();
			dataPath = await appDataDir();
		} catch (error) {
			console.error("Failed to load app info:", error);
		}
	}

	// Theme handling
	async function handleThemeChange(value: string) {
		if (value === "system") {
			await setTheme(null);
			resetMode();
		} else {
			await setTheme(value as "light" | "dark");
			setMode(value as "light" | "dark");
		}
	}

	const currentTheme = $derived(
		mode.current === "light" ? "light" : mode.current === "dark" ? "dark" : "system"
	);

	const themeLabel = $derived(
		currentTheme === "light"
			? m.theme_light()
			: currentTheme === "dark"
				? m.theme_dark()
				: m.theme_system()
	);

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

	const navGroups: NavGroup[] = [
		{
			id: "general",
			name: m.settings_general(),
			icon: SettingsIcon,
			items: [
				{ id: "app-info", name: m.settings_app_info(), icon: InfoIcon },
			],
		},
		{
			id: "appearance",
			name: m.settings_appearance(),
			icon: PaletteIcon,
			items: [
				{ id: "theme", name: m.settings_theme(), icon: SunMoonIcon },
			],
		},
	];

	// Find the active group for breadcrumb display
	const activeGroup = $derived(() => {
		const groupId = settingsDialogStore.getActiveGroup();
		return navGroups.find((g) => g.id === groupId);
	});

	// Find the active section name (only when viewing a specific section)
	const activeSectionName = $derived(() => {
		if (settingsDialogStore.isGroupView()) return null;
		for (const group of navGroups) {
			const item = group.items.find((item) => item.id === settingsDialogStore.activeView);
			if (item) return item.name;
		}
		return null;
	});

	// Check if a section should be shown
	function shouldShowSection(sectionId: SettingsSection): boolean {
		const view = settingsDialogStore.activeView;
		// If viewing a specific section, only show that one
		if (view === sectionId) return true;
		// If viewing a group, show all sections in that group
		if (view === "general" || view === "appearance") {
			return groupSections[view].includes(sectionId);
		}
		return false;
	}

	// Check if a menu item is active
	function isItemActive(itemId: SettingsSection): boolean {
		const view = settingsDialogStore.activeView;
		if (view === itemId) return true;
		// Also highlight if viewing the parent group
		if (view === "general") return itemId === "app-info";
		if (view === "appearance") return itemId === "theme";
		return false;
	}
</script>

<Dialog.Root bind:open={settingsDialogStore.isOpen}>
	<Dialog.Content
		class="overflow-hidden p-0 md:max-h-[500px] md:max-w-[700px] lg:max-w-[800px]"
		trapFocus={false}
	>
		<Dialog.Title class="sr-only">{m.settings_title()}</Dialog.Title>
		<Dialog.Description class="sr-only">Customize your settings here.</Dialog.Description>
		<Sidebar.Provider class="items-start">
			<Sidebar.Root collapsible="none" class="hidden md:flex">
				<Sidebar.Content>
					{#each navGroups as group (group.id)}
						<Sidebar.Group>
							<Sidebar.GroupLabel
								class="gap-2 cursor-pointer hover:text-foreground transition-colors"
								onclick={() => settingsDialogStore.setView(group.id)}
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
												onclick={() => settingsDialogStore.setView(item.id)}
											>
												<item.icon class="size-4" />
												<span>{item.name}</span>
											</Sidebar.MenuButton>
										</Sidebar.MenuItem>
									{/each}
								</Sidebar.Menu>
							</Sidebar.GroupContent>
						</Sidebar.Group>
					{/each}
				</Sidebar.Content>
			</Sidebar.Root>
			<main class="flex h-[480px] flex-1 flex-col overflow-hidden">
				<header
					class="flex h-16 shrink-0 items-center gap-2 transition-[width,height] ease-linear group-has-[[data-collapsible=icon]]/sidebar-wrapper:h-12"
				>
					<div class="flex items-center gap-2 px-4">
						<Breadcrumb.Root>
							<Breadcrumb.List>
								<Breadcrumb.Item class="hidden md:block">
									<Breadcrumb.Link href="#" onclick={() => settingsDialogStore.setView("general")}>
										{m.settings_title()}
									</Breadcrumb.Link>
								</Breadcrumb.Item>
								<Breadcrumb.Separator class="hidden md:block" />
								<Breadcrumb.Item class="hidden md:block">
									{#if activeSectionName()}
										<Breadcrumb.Link
											href="#"
											onclick={() => settingsDialogStore.setView(activeGroup()?.id ?? "general")}
										>
											{activeGroup()?.name}
										</Breadcrumb.Link>
									{:else}
										<Breadcrumb.Page>{activeGroup()?.name}</Breadcrumb.Page>
									{/if}
								</Breadcrumb.Item>
								{#if activeSectionName()}
									<Breadcrumb.Separator class="hidden md:block" />
									<Breadcrumb.Item>
										<Breadcrumb.Page>{activeSectionName()}</Breadcrumb.Page>
									</Breadcrumb.Item>
								{/if}
							</Breadcrumb.List>
						</Breadcrumb.Root>
					</div>
				</header>
				<div class="flex flex-1 flex-col gap-6 overflow-y-auto p-4 pt-0">
					{#if shouldShowSection("app-info")}
						<div class="space-y-6">
							<div>
								<h2 class="text-lg font-medium">{m.settings_app_info()}</h2>
								<p class="text-sm text-muted-foreground mt-1">
									Information about your Seaquel installation
								</p>
							</div>

							<div class="space-y-4">
								<div class="grid grid-cols-[140px_1fr] gap-2 text-sm">
									<span class="text-muted-foreground">{m.settings_version()}</span>
									<span class="font-mono">{appVersion || "..."}</span>
								</div>
								<div class="grid grid-cols-[140px_1fr] gap-2 text-sm">
									<span class="text-muted-foreground">{m.settings_config_dir()}</span>
									<span class="font-mono text-xs break-all select-all">{configPath || "..."}</span>
								</div>
								<div class="grid grid-cols-[140px_1fr] gap-2 text-sm">
									<span class="text-muted-foreground">{m.settings_data_dir()}</span>
									<span class="font-mono text-xs break-all select-all">{dataPath || "..."}</span>
								</div>
							</div>
						</div>
					{/if}

					{#if shouldShowSection("theme")}
						<div class="space-y-6">
							<div>
								<h2 class="text-lg font-medium">{m.settings_theme()}</h2>
								<p class="text-sm text-muted-foreground mt-1">
									{m.settings_theme_description()}
								</p>
							</div>

							<div class="space-y-4">
								<div class="flex items-center justify-between">
									<div>
										<p class="text-sm font-medium">{m.settings_theme_label()}</p>
										<p class="text-xs text-muted-foreground">
											Choose between light, dark, or system theme
										</p>
									</div>
									<Select
										type="single"
										value={currentTheme}
										onValueChange={handleThemeChange}
									>
										<SelectTrigger class="w-32">
											{themeLabel}
										</SelectTrigger>
										<SelectContent>
											<SelectItem value="light">{m.theme_light()}</SelectItem>
											<SelectItem value="dark">{m.theme_dark()}</SelectItem>
											<SelectItem value="system">{m.theme_system()}</SelectItem>
										</SelectContent>
									</Select>
								</div>
							</div>
						</div>
					{/if}
				</div>
			</main>
		</Sidebar.Provider>
	</Dialog.Content>
</Dialog.Root>
