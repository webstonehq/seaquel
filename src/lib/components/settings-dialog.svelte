<script lang="ts">
	import { m } from "$lib/paraglide/messages.js";
	import * as Breadcrumb from "$lib/components/ui/breadcrumb/index.js";
	import * as Dialog from "$lib/components/ui/dialog/index.js";
	import * as Sidebar from "$lib/components/ui/sidebar/index.js";
	import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";
	import {
		Select,
		SelectContent,
		SelectItem,
		SelectTrigger,
	} from "$lib/components/ui/select";
	import { Button } from "$lib/components/ui/button";
	import {
		settingsDialogStore,
		type SettingsSection,
		type SettingsGroup,
		type SettingsView,
		groupSections,
	} from "$lib/stores/settings-dialog.svelte.js";
	import { themeStore } from "$lib/stores/theme.svelte.js";
	import { getVersion } from "@tauri-apps/api/app";
	import { appConfigDir, appDataDir } from "@tauri-apps/api/path";
	import { setMode, resetMode, mode } from "mode-watcher";
	import { setTheme } from "@tauri-apps/api/app";
	import { toast } from "svelte-sonner";
import { errorToast } from "$lib/utils/toast";
	import { open, save } from "@tauri-apps/plugin-dialog";
	import { readTextFile, writeTextFile } from "@tauri-apps/plugin-fs";
	import SettingsIcon from "@lucide/svelte/icons/settings";
	import PaletteIcon from "@lucide/svelte/icons/palette";
	import InfoIcon from "@lucide/svelte/icons/info";
	import SunMoonIcon from "@lucide/svelte/icons/sun-moon";
	import SwatchBookIcon from "@lucide/svelte/icons/swatch-book";
	import PlusIcon from "@lucide/svelte/icons/plus";
	import UploadIcon from "@lucide/svelte/icons/upload";
	import CopyIcon from "@lucide/svelte/icons/copy";
	import DownloadIcon from "@lucide/svelte/icons/download";
	import TrashIcon from "@lucide/svelte/icons/trash-2";
	import CheckIcon from "@lucide/svelte/icons/check";
	import ThemePreview from "./theme-preview.svelte";
	import { openThemeEditor } from "$lib/utils/theme-editor-window";
	import type { Theme } from "$lib/types/theme";

	// App info state
	let appVersion = $state<string>("");
	let configPath = $state<string>("");
	let dataPath = $state<string>("");

	// Delete confirmation state
	let deleteDialogOpen = $state(false);
	let themeToDelete = $state<Theme | null>(null);

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

	// Mode handling (light/dark/system)
	async function handleModeChange(value: string) {
		if (value === "system") {
			await setTheme(null);
			resetMode();
		} else {
			await setTheme(value as "light" | "dark");
			setMode(value as "light" | "dark");
		}
	}

	const currentMode = $derived(
		mode.current === "light" ? "light" : mode.current === "dark" ? "dark" : "system"
	);

	const modeLabel = $derived(
		currentMode === "light"
			? m.theme_light()
			: currentMode === "dark"
				? m.theme_dark()
				: m.theme_system()
	);

	// Theme selection
	function handleLightThemeChange(themeId: string) {
		themeStore.setLightTheme(themeId);
	}

	function handleDarkThemeChange(themeId: string) {
		themeStore.setDarkTheme(themeId);
	}

	// Theme actions
	function openCreateTheme() {
		openThemeEditor(null);
	}

	function openEditTheme(theme: Theme) {
		openThemeEditor(theme);
	}

	function duplicateTheme(theme: Theme) {
		const newTheme = themeStore.duplicateTheme(theme.id);
		if (newTheme) {
			toast.success(m.theme_duplicate_success());
		}
	}

	async function exportTheme(theme: Theme) {
		try {
			const json = themeStore.exportTheme(theme.id);
			const fileName = theme.name.toLowerCase().replace(/\s+/g, "-") + ".json";

			const filePath = await save({
				defaultPath: fileName,
				filters: [{ name: "JSON", extensions: ["json"] }],
			});

			if (filePath) {
				await writeTextFile(filePath, json);
				toast.success(m.theme_export_success());
			}
		} catch (error) {
			console.error("Failed to export theme:", error);
		}
	}

	function confirmDeleteTheme(theme: Theme) {
		themeToDelete = theme;
		deleteDialogOpen = true;
	}

	function deleteTheme() {
		if (themeToDelete) {
			themeStore.deleteTheme(themeToDelete.id);
			toast.success(m.theme_delete_success());
			themeToDelete = null;
			deleteDialogOpen = false;
		}
	}

	async function importTheme() {
		try {
			const filePath = await open({
				filters: [{ name: "JSON", extensions: ["json"] }],
				multiple: false,
			});

			if (filePath) {
				const content = await readTextFile(filePath as string);
				themeStore.importTheme(content);
				toast.success(m.theme_import_success());
			}
		} catch (error) {
			const message = error instanceof Error ? error.message : String(error);
			errorToast(m.theme_import_error({ error: message }));
		}
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
				{ id: "themes", name: m.settings_themes(), icon: SwatchBookIcon },
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
		// Also highlight first item if viewing the parent group
		if (view === "general") return itemId === "app-info";
		if (view === "appearance") return itemId === "theme";
		return false;
	}

	// Theme display helpers
	const lightThemeLabel = $derived(themeStore.selectedLightTheme.name);
	const darkThemeLabel = $derived(themeStore.selectedDarkTheme.name);
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
											Choose between light, dark, or system mode
										</p>
									</div>
									<Select
										type="single"
										value={currentMode}
										onValueChange={handleModeChange}
									>
										<SelectTrigger class="w-32">
											{modeLabel}
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

					{#if shouldShowSection("themes")}
						<div class="space-y-6">
							<div>
								<h2 class="text-lg font-medium">{m.settings_themes()}</h2>
								<p class="text-sm text-muted-foreground mt-1">
									{m.settings_themes_description()}
								</p>
							</div>

							<!-- Theme Selection -->
							<div class="space-y-4">
								<!-- Light Mode Theme -->
								<div class="flex items-center justify-between">
									<div>
										<p class="text-sm font-medium">{m.settings_themes_light_mode()}</p>
										<p class="text-xs text-muted-foreground">
											{m.settings_themes_light_mode_description()}
										</p>
									</div>
									<Select
										type="single"
										value={themeStore.preferences.lightThemeId}
										onValueChange={handleLightThemeChange}
									>
										<SelectTrigger class="w-48">
											{lightThemeLabel}
										</SelectTrigger>
										<SelectContent>
											{#each themeStore.lightThemes as theme (theme.id)}
												<SelectItem value={theme.id}>{theme.name}</SelectItem>
											{/each}
										</SelectContent>
									</Select>
								</div>

								<!-- Dark Mode Theme -->
								<div class="flex items-center justify-between">
									<div>
										<p class="text-sm font-medium">{m.settings_themes_dark_mode()}</p>
										<p class="text-xs text-muted-foreground">
											{m.settings_themes_dark_mode_description()}
										</p>
									</div>
									<Select
										type="single"
										value={themeStore.preferences.darkThemeId}
										onValueChange={handleDarkThemeChange}
									>
										<SelectTrigger class="w-48">
											{darkThemeLabel}
										</SelectTrigger>
										<SelectContent>
											{#each themeStore.darkThemes as theme (theme.id)}
												<SelectItem value={theme.id}>{theme.name}</SelectItem>
											{/each}
										</SelectContent>
									</Select>
								</div>
							</div>

							<!-- User Themes -->
							<div class="space-y-3">
								<div class="flex items-center justify-between">
									<h3 class="text-sm font-medium">{m.settings_themes_user_themes()}</h3>
									<div class="flex gap-2">
										<Button variant="outline" size="sm" onclick={importTheme}>
											<UploadIcon class="size-4 mr-1" />
											{m.settings_themes_import()}
										</Button>
										<Button variant="outline" size="sm" onclick={openCreateTheme}>
											<PlusIcon class="size-4 mr-1" />
											{m.settings_themes_create_new()}
										</Button>
									</div>
								</div>

								{#if themeStore.userThemes.length === 0}
									<div class="text-center py-6 border rounded-lg bg-muted/30">
										<p class="text-sm text-muted-foreground">{m.settings_themes_no_user_themes()}</p>
										<p class="text-xs text-muted-foreground mt-1">{m.settings_themes_no_user_themes_hint()}</p>
									</div>
								{:else}
									<div class="space-y-2">
										{#each themeStore.userThemes as theme (theme.id)}
											<div class="flex items-center justify-between p-3 border rounded-lg hover:bg-muted/30 transition-colors">
												<div class="flex items-center gap-3">
													<ThemePreview colors={theme.colors} />
													<div>
														<div class="flex items-center gap-2">
															<span class="text-sm font-medium">{theme.name}</span>
															{#if themeStore.isThemeActive(theme.id)}
																<span class="text-xs bg-primary/10 text-primary px-1.5 py-0.5 rounded">
																	{m.theme_card_active()}
																</span>
															{/if}
														</div>
														<span class="text-xs text-muted-foreground">
															{theme.isDark ? m.theme_dark() : m.theme_light()}
														</span>
													</div>
												</div>
												<div class="flex items-center gap-1">
													<Button variant="ghost" size="icon" class="size-8" onclick={() => openEditTheme(theme)} title={m.theme_card_edit()}>
														<PaletteIcon class="size-4" />
													</Button>
													<Button variant="ghost" size="icon" class="size-8" onclick={() => duplicateTheme(theme)} title={m.theme_card_duplicate()}>
														<CopyIcon class="size-4" />
													</Button>
													<Button variant="ghost" size="icon" class="size-8" onclick={() => exportTheme(theme)} title={m.theme_card_export()}>
														<DownloadIcon class="size-4" />
													</Button>
													<Button variant="ghost" size="icon" class="size-8 text-destructive hover:text-destructive" onclick={() => confirmDeleteTheme(theme)} title={m.theme_card_delete()}>
														<TrashIcon class="size-4" />
													</Button>
												</div>
											</div>
										{/each}
									</div>
								{/if}
							</div>

							<!-- Built-in Themes -->
							<div class="space-y-3">
								<h3 class="text-sm font-medium">{m.settings_themes_builtin_themes()}</h3>
								<div class="space-y-2">
									{#each themeStore.allThemes.filter(t => t.isBuiltIn) as theme (theme.id)}
										<div class="flex items-center justify-between p-3 border rounded-lg hover:bg-muted/30 transition-colors">
											<div class="flex items-center gap-3">
												<ThemePreview colors={theme.colors} />
												<div>
													<div class="flex items-center gap-2">
														<span class="text-sm font-medium">{theme.name}</span>
														{#if themeStore.isThemeActive(theme.id)}
															<span class="text-xs bg-primary/10 text-primary px-1.5 py-0.5 rounded">
																{m.theme_card_active()}
															</span>
														{/if}
													</div>
													<span class="text-xs text-muted-foreground">
														{theme.isDark ? m.theme_dark() : m.theme_light()}
														{#if theme.author}
															&middot; {theme.author}
														{/if}
													</span>
												</div>
											</div>
											<div class="flex items-center gap-1">
												<Button variant="ghost" size="icon" class="size-8" onclick={() => duplicateTheme(theme)} title={m.theme_card_duplicate()}>
													<CopyIcon class="size-4" />
												</Button>
											</div>
										</div>
									{/each}
								</div>
							</div>
						</div>
					{/if}
				</div>
			</main>
		</Sidebar.Provider>
	</Dialog.Content>
</Dialog.Root>

<!-- Delete Confirmation Dialog -->
<AlertDialog.Root bind:open={deleteDialogOpen}>
	<AlertDialog.Content>
		<AlertDialog.Header>
			<AlertDialog.Title>{m.theme_delete_title()}</AlertDialog.Title>
			<AlertDialog.Description>
				{m.theme_delete_description({ name: themeToDelete?.name ?? "" })}
			</AlertDialog.Description>
		</AlertDialog.Header>
		<AlertDialog.Footer>
			<AlertDialog.Cancel>{m.theme_delete_cancel()}</AlertDialog.Cancel>
			<AlertDialog.Action onclick={deleteTheme} class="bg-destructive text-white hover:bg-destructive/90">
				{m.theme_delete_confirm()}
			</AlertDialog.Action>
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>
