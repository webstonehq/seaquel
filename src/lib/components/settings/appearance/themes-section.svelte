<script lang="ts">
	import { m } from "$lib/paraglide/messages.js";
	import {
		Select,
		SelectContent,
		SelectItem,
		SelectTrigger,
	} from "$lib/components/ui/select";
	import { Button } from "$lib/components/ui/button";
	import DeleteConfirmDialog from "$lib/components/delete-confirm-dialog.svelte";
	import { themeStore } from "$lib/stores/theme.svelte.js";
	import { toast } from "svelte-sonner";
	import { errorToast } from "$lib/utils/toast";
	import { open, save } from "@tauri-apps/plugin-dialog";
	import { readTextFile, writeTextFile } from "@tauri-apps/plugin-fs";
	import PaletteIcon from "@lucide/svelte/icons/palette";
	import PlusIcon from "@lucide/svelte/icons/plus";
	import UploadIcon from "@lucide/svelte/icons/upload";
	import CopyIcon from "@lucide/svelte/icons/copy";
	import DownloadIcon from "@lucide/svelte/icons/download";
	import TrashIcon from "@lucide/svelte/icons/trash-2";
	import ThemePreview from "$lib/components/theme-preview.svelte";
	import { openThemeEditor } from "$lib/utils/theme-editor-window";
	import type { Theme } from "$lib/types/theme";

	let deleteDialogOpen = $state(false);
	let themeToDelete = $state<Theme | null>(null);

	function confirmDeleteTheme(theme: Theme) {
		themeToDelete = theme;
		deleteDialogOpen = true;
	}

	async function deleteTheme() {
		if (themeToDelete) {
			const id = themeToDelete.id;
			themeToDelete = null;
			deleteDialogOpen = false;
			// A failure is shown by the store.
			await themeStore.deleteTheme(id);
			if (!themeStore.userThemes.some((t) => t.id === id)) {
				toast.success(m.theme_delete_success());
			}
		}
	}

	const lightThemeLabel = $derived(themeStore.selectedLightTheme.name);
	const darkThemeLabel = $derived(themeStore.selectedDarkTheme.name);

	function handleLightThemeChange(themeId: string) {
		void themeStore.setLightTheme(themeId);
	}

	function handleDarkThemeChange(themeId: string) {
		void themeStore.setDarkTheme(themeId);
	}

	function openCreateTheme() {
		openThemeEditor(null);
	}

	function openEditTheme(theme: Theme) {
		openThemeEditor(theme);
	}

	async function duplicateTheme(theme: Theme) {
		const newTheme = await themeStore.duplicateTheme(theme.id);
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

	async function importTheme() {
		try {
			const filePath = await open({
				filters: [{ name: "JSON", extensions: ["json"] }],
				multiple: false,
			});

			if (filePath) {
				const content = await readTextFile(filePath as string);
				// A theme Core refused is shown by the store.
				if (await themeStore.importTheme(content)) toast.success(m.theme_import_success());
			}
		} catch (error) {
			const message = error instanceof Error ? error.message : String(error);
			errorToast(m.theme_import_error({ error: message }));
		}
	}
</script>

<div class="space-y-6" data-section="themes">
	<div>
		<h2 class="text-lg font-medium">{m.settings_themes()}</h2>
		<p class="text-sm text-muted-foreground mt-1">
			{m.settings_themes_description()}
		</p>
	</div>

	<div class="space-y-4">
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

<DeleteConfirmDialog
	bind:open={deleteDialogOpen}
	title={m.theme_delete_title()}
	description={m.theme_delete_description({ name: themeToDelete?.name ?? "" })}
	cancelText={m.theme_delete_cancel()}
	confirmText={m.theme_delete_confirm()}
	onconfirm={deleteTheme}
/>
