<script lang="ts">
	import { m } from "$lib/paraglide/messages.js";
	import * as Dialog from "$lib/components/ui/dialog/index.js";
	import * as Tabs from "$lib/components/ui/tabs/index.js";
	import { Button } from "$lib/components/ui/button";
	import { Input } from "$lib/components/ui/input";
	import { Label } from "$lib/components/ui/label";
	import { Switch } from "$lib/components/ui/switch";
	import { ScrollArea } from "$lib/components/ui/scroll-area";
	import { toast } from "svelte-sonner";
	import { themeStore } from "$lib/stores/theme.svelte.js";
	import { BUILT_IN_THEMES } from "$lib/themes/presets";
	import { getColorLabel } from "$lib/themes/color-utils";
	import { applyThemeColors } from "$lib/themes/apply";
	import { COLOR_GROUPS, type Theme, type ThemeColors } from "$lib/types/theme";
	import ColorPicker from "./color-picker.svelte";
	import ThemePreview from "./theme-preview.svelte";

	interface Props {
		open: boolean;
		editTheme: Theme | null;
	}

	let { open = $bindable(), editTheme }: Props = $props();

	// Form state
	let themeName = $state("");
	let isDark = $state(false);
	let colors = $state<ThemeColors>({ ...BUILT_IN_THEMES[0].colors });

	// Store original theme to restore on cancel
	let originalColors: ThemeColors | null = null;
	// Track if we're saving (to avoid restoring colors on save)
	let isSaving = false;

	// Reset form when dialog opens
	$effect(() => {
		if (open) {
			// Store the current theme colors to restore on cancel
			originalColors = { ...themeStore.activeTheme.colors };

			if (editTheme) {
				// Editing existing theme
				themeName = editTheme.name;
				isDark = editTheme.isDark;
				colors = { ...editTheme.colors };
			} else {
				// Creating new theme - start with current active theme colors
				themeName = "";
				isDark = themeStore.activeTheme.isDark;
				colors = { ...themeStore.activeTheme.colors };
			}
		}
	});

	// Apply colors in real-time as they change
	$effect(() => {
		if (open) {
			applyThemeColors(colors);
		}
	});

	const isEditing = $derived(editTheme !== null);
	const dialogTitle = $derived(isEditing ? m.theme_editor_title_edit() : m.theme_editor_title_new());

	function updateColor(key: keyof ThemeColors, value: string) {
		colors = { ...colors, [key]: value };
	}

	function handleSave() {
		if (!themeName.trim()) {
			toast.error("Please enter a theme name");
			return;
		}

		if (isEditing && editTheme) {
			themeStore.updateTheme(editTheme.id, {
				name: themeName.trim(),
				isDark,
				colors,
			});
		} else {
			themeStore.addTheme({
				name: themeName.trim(),
				isDark,
				colors,
			});
		}

		toast.success(m.theme_save_success());
		isSaving = true;
		open = false;
	}

	function handleCancel() {
		// Restore original theme colors
		if (originalColors) {
			applyThemeColors(originalColors);
		}
		open = false;
	}

	function handleOpenChange(isOpen: boolean) {
		if (!isOpen) {
			// Dialog is closing - restore colors unless we're saving
			if (!isSaving && originalColors) {
				applyThemeColors(originalColors);
			}
			// Reset flags
			isSaving = false;
			originalColors = null;
		}
	}

	// Get color group name for display
	function getGroupName(groupName: string): string {
		switch (groupName) {
			case "Background": return m.theme_editor_group_background();
			case "Interactive": return m.theme_editor_group_interactive();
			case "Status": return m.theme_editor_group_status();
			case "Borders": return m.theme_editor_group_borders();
			case "Charts": return m.theme_editor_group_charts();
			case "Sidebar": return m.theme_editor_group_sidebar();
			default: return groupName;
		}
	}
</script>

<Dialog.Root bind:open onOpenChange={handleOpenChange}>
	<Dialog.Content class="max-w-3xl max-h-[85vh] flex flex-col">
		<Dialog.Header>
			<Dialog.Title>{dialogTitle}</Dialog.Title>
			<Dialog.Description>
				{#if isEditing}
					Edit your custom theme colors
				{:else}
					Create a new theme with custom colors
				{/if}
			</Dialog.Description>
		</Dialog.Header>

		<div class="flex-1 min-h-0 py-4">
			<div class="grid grid-cols-[1fr_200px] gap-6 h-full">
				<!-- Left side: Form -->
				<div class="flex flex-col gap-4 min-h-0">
					<!-- Theme Name & Mode -->
					<div class="grid grid-cols-2 gap-4">
						<div class="space-y-2">
							<Label for="theme-name">{m.theme_editor_name()}</Label>
							<Input
								id="theme-name"
								bind:value={themeName}
								placeholder={m.theme_editor_name_placeholder()}
							/>
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
						<ScrollArea class="h-[350px] border rounded-lg p-3">
							<Tabs.Root value="Background" class="w-full">
								<Tabs.List class="w-full justify-start flex-wrap h-auto gap-1 mb-4">
									{#each COLOR_GROUPS as group}
										<Tabs.Trigger value={group.name} class="text-xs px-2 py-1">
											{getGroupName(group.name)}
										</Tabs.Trigger>
									{/each}
								</Tabs.List>

								{#each COLOR_GROUPS as group}
									<Tabs.Content value={group.name} class="space-y-3">
										{#each group.keys as key}
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
						class="border rounded-lg p-4 h-[calc(100%-2rem)]"
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
								<div
									class="text-xs"
									style="color: {colors.mutedForeground}"
								>
									Card description text
								</div>
							</div>

							<!-- Button previews -->
							<div class="flex gap-2">
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
							<div
								class="rounded px-2 py-1 text-xs inline-block"
								style="background-color: {colors.destructive}; color: white;"
							>
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
						</div>
					</div>
				</div>
			</div>
		</div>

		<Dialog.Footer>
			<Button variant="outline" onclick={handleCancel}>
				{m.theme_editor_cancel()}
			</Button>
			<Button onclick={handleSave}>
				{m.theme_editor_save()}
			</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>
