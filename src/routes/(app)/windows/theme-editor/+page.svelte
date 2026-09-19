<script lang="ts">
	import { page } from "$app/state";
	import { m } from "$lib/paraglide/messages.js";
	import * as Tabs from "$lib/components/ui/tabs/index.js";
	import { Button } from "$lib/components/ui/button";
	import { Input } from "$lib/components/ui/input";
	import { Label } from "$lib/components/ui/label";
	import { Switch } from "$lib/components/ui/switch";
	import { ScrollArea } from "$lib/components/ui/scroll-area";
	import { themeStore } from "$lib/stores/theme.svelte.js";
	import { BUILT_IN_THEMES } from "$lib/themes/presets";
	import { getColorLabel } from "$lib/themes/color-utils";
	import { applyThemeColors } from "$lib/themes/apply";
	import { COLOR_GROUPS, type Theme, type ThemeColors } from "$lib/types/theme";
	import ColorPicker from "$lib/components/color-picker.svelte";
	import { emit } from "@tauri-apps/api/event";
	import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
	import { onMount } from "svelte";

	// Get theme ID from URL params
	const themeId = $derived(page.url.searchParams.get("themeId"));
	const isEditing = $derived(themeId !== null);

	// Form state
	let themeName = $state("");
	let isDark = $state(false);
	let colors = $state<ThemeColors>({ ...BUILT_IN_THEMES[0].colors });

	// Store original theme to restore on cancel
	let originalColors: ThemeColors | null = null;
	let initialized = $state(false);

	// Initialize when store is loaded
	$effect(() => {
		if (themeStore.isLoaded && !initialized) {
			// Store the current theme colors to restore on cancel
			originalColors = { ...themeStore.activeTheme.colors };

			if (themeId) {
				// Editing existing theme
				const editTheme = themeStore.getTheme(themeId);
				if (editTheme) {
					themeName = editTheme.name;
					isDark = editTheme.isDark;
					colors = { ...editTheme.colors };
				}
			} else {
				// Creating new theme - start with current active theme colors
				themeName = "";
				isDark = themeStore.activeTheme.isDark;
				colors = { ...themeStore.activeTheme.colors };
			}
			initialized = true;
		}
	});

	// Apply colors in real-time and emit to main window
	$effect(() => {
		if (initialized) {
			applyThemeColors(colors);
			// Emit to main window for real-time preview
			emit("theme-editor:color-update", { colors });
		}
	});

	// Handle window close button (treat as cancel)
	onMount(() => {
		const currentWindow = getCurrentWebviewWindow();
		const unlisten = currentWindow.onCloseRequested(async (event) => {
			// Prevent default close behavior so we can emit cancel first
			event.preventDefault();
			// Emit cancel event
			await emit("theme-editor:cancel", {});
			// Now destroy the window
			await currentWindow.destroy();
		});

		return () => {
			unlisten.then((fn) => fn());
		};
	});

	const dialogTitle = $derived(isEditing ? m.theme_editor_title_edit() : m.theme_editor_title_new());

	function updateColor(key: keyof ThemeColors, value: string) {
		colors = { ...colors, [key]: value };
	}

	async function handleSave() {
		if (!themeName.trim()) {
			// Show inline error
			return;
		}

		// Emit save event to main window
		await emit("theme-editor:save", {
			themeId: themeId,
			name: themeName.trim(),
			isDark,
			colors,
		});

		// Destroy the window (use destroy to skip onCloseRequested)
		const currentWindow = getCurrentWebviewWindow();
		await currentWindow.destroy();
	}

	async function handleCancel() {
		// Restore original theme colors locally
		if (originalColors) {
			applyThemeColors(originalColors);
		}
		// Emit cancel event
		await emit("theme-editor:cancel", {});
		// Destroy the window (use destroy to skip onCloseRequested)
		const currentWindow = getCurrentWebviewWindow();
		await currentWindow.destroy();
	}

	// Get color group name for display
	function getGroupName(groupName: string): string {
		switch (groupName) {
			case "Background":
				return m.theme_editor_group_background();
			case "Interactive":
				return m.theme_editor_group_interactive();
			case "Status":
				return m.theme_editor_group_status();
			case "Borders":
				return m.theme_editor_group_borders();
			case "Charts":
				return m.theme_editor_group_charts();
			case "Sidebar":
				return m.theme_editor_group_sidebar();
			default:
				return groupName;
		}
	}
</script>

<div class="flex flex-col h-screen">
	<!-- Header with drag region -->
	<header class="h-12 flex items-center justify-between px-4 border-b bg-background" data-tauri-drag-region>
		<h1 class="text-sm font-medium">{dialogTitle}</h1>
		<div class="flex gap-2">
			<Button variant="outline" size="sm" onclick={handleCancel}>
				{m.theme_editor_cancel()}
			</Button>
			<Button size="sm" onclick={handleSave}>
				{m.theme_editor_save()}
			</Button>
		</div>
	</header>

	<!-- Content -->
	<div class="flex-1 min-h-0 p-4">
		<div class="grid grid-cols-[1fr_220px] gap-6 h-full">
			<!-- Left side: Form -->
			<div class="flex flex-col gap-4 min-h-0">
				<!-- Theme Name & Mode -->
				<div class="grid grid-cols-2 gap-4">
					<div class="space-y-2">
						<Label for="theme-name">{m.theme_editor_name()}</Label>
						<Input id="theme-name" bind:value={themeName} placeholder={m.theme_editor_name_placeholder()} />
					</div>
					<div class="space-y-2">
						<Label>{m.theme_editor_mode()}</Label>
						<div class="flex items-center gap-3 h-9">
							<span class="text-sm text-muted-foreground">{m.theme_editor_mode_light()}</span>
							<Switch bind:checked={isDark} />
							<span class="text-sm text-muted-foreground">{m.theme_editor_mode_dark()}</span>
						</div>
					</div>
				</div>

				<!-- Color Groups -->
				<div class="flex-1 min-h-0">
					<Label class="mb-2 block">{m.theme_editor_colors()}</Label>
					<ScrollArea class="h-[calc(100%-2rem)] border rounded-lg p-3">
						<Tabs.Root value="Background" class="w-full">
							<Tabs.List class="w-full justify-start flex-wrap h-auto gap-1 mb-4">
								{#each COLOR_GROUPS as group (group.name)}
									<Tabs.Trigger value={group.name} class="text-xs px-2 py-1">
										{getGroupName(group.name)}
									</Tabs.Trigger>
								{/each}
							</Tabs.List>

							{#each COLOR_GROUPS as group (group.name)}
								<Tabs.Content value={group.name} class="space-y-3">
									{#each group.keys as key (key)}
										<ColorPicker
											value={colors[key as keyof ThemeColors]}
											label={getColorLabel(key as keyof ThemeColors)}
											onchange={(v) => updateColor(key as keyof ThemeColors, v)}
										/>
									{/each}
								</Tabs.Content>
							{/each}
						</Tabs.Root>
					</ScrollArea>
				</div>
			</div>

			<!-- Right side: Preview -->
			<div class="space-y-2">
				<Label>{m.theme_editor_preview()}</Label>
				<div
					class="border rounded-lg p-4 h-[calc(100%-2rem)] overflow-y-auto"
					style="
						background-color: {colors.background};
						color: {colors.foreground};
					"
				>
					<div class="space-y-3">
						<!-- Card preview -->
						<div
							class="rounded-lg border p-3"
							style="
								background-color: {colors.card};
								color: {colors.cardForeground};
								border-color: {colors.border};
							"
						>
							<div class="text-sm font-medium mb-1">Card Title</div>
							<div class="text-xs" style="color: {colors.mutedForeground}">Card description text</div>
						</div>

						<!-- Button previews -->
						<div class="flex gap-2 flex-wrap">
							<div
								class="rounded px-2 py-1 text-xs"
								style="
									background-color: {colors.primary};
									color: {colors.primaryForeground};
								"
							>
								Primary
							</div>
							<div
								class="rounded px-2 py-1 text-xs"
								style="
									background-color: {colors.secondary};
									color: {colors.secondaryForeground};
								"
							>
								Secondary
							</div>
						</div>

						<!-- Accent preview -->
						<div
							class="rounded px-2 py-1 text-xs inline-block"
							style="
								background-color: {colors.accent};
								color: {colors.accentForeground};
							"
						>
							Accent
						</div>

						<!-- Destructive preview -->
						<div class="rounded px-2 py-1 text-xs inline-block" style="background-color: {colors.destructive}; color: white;">
							Destructive
						</div>

						<!-- Input preview -->
						<div
							class="rounded border px-2 py-1 text-xs"
							style="
								background-color: {colors.background};
								border-color: {colors.input};
								color: {colors.foreground};
							"
						>
							Input field
						</div>

						<!-- Sidebar preview -->
						<div
							class="rounded-lg p-2 text-xs"
							style="
								background-color: {colors.sidebar};
								color: {colors.sidebarForeground};
							"
						>
							<div class="mb-1">Sidebar</div>
							<div
								class="rounded px-1 py-0.5"
								style="
									background-color: {colors.sidebarAccent};
									color: {colors.sidebarAccentForeground};
								"
							>
								Active item
							</div>
						</div>

						<!-- Chart colors preview -->
						<div class="flex gap-1">
							<div class="w-4 h-4 rounded" style="background-color: {colors.chart1}"></div>
							<div class="w-4 h-4 rounded" style="background-color: {colors.chart2}"></div>
							<div class="w-4 h-4 rounded" style="background-color: {colors.chart3}"></div>
							<div class="w-4 h-4 rounded" style="background-color: {colors.chart4}"></div>
							<div class="w-4 h-4 rounded" style="background-color: {colors.chart5}"></div>
						</div>
					</div>
				</div>
			</div>
		</div>
	</div>
</div>
