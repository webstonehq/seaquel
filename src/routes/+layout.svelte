<script lang="ts">
	import { page } from '$app/state';
	import { locales, localizeHref } from '$lib/paraglide/runtime';
	import "./layout.css";
	import { ModeWatcher } from "mode-watcher";
	import { Toaster } from "$lib/components/ui/sonner/index.js";
	import AppHeader from "$lib/components/app-header.svelte";
	import * as Sidebar from "$lib/components/ui/sidebar/index.js";
	import { setDatabase, useDatabase } from "$lib/hooks/database.svelte.js";
	import { setShortcuts } from "$lib/shortcuts/index.js";
	import KeyboardShortcutsDialog from "$lib/components/keyboard-shortcuts-dialog.svelte";
	import CommandPalette from "$lib/components/command-palette.svelte";
	import SettingsDialog from "$lib/components/settings-dialog.svelte";
	import { settingsDialogStore } from "$lib/stores/settings-dialog.svelte.js";
	import { themeStore } from "$lib/stores/theme.svelte.js";
	import { applyThemeColors } from "$lib/themes/apply";
	import DbeaverImportDialog from "$lib/components/dbeaver-import-dialog.svelte";
	import type { ThemeColors } from "$lib/types/theme";
	import { listen } from "@tauri-apps/api/event";
	import { invoke } from "@tauri-apps/api/core";
	import { toast } from "svelte-sonner";
	import { m } from "$lib/paraglide/messages.js";
	import { onMount } from "svelte";
	import { onboardingStore } from "$lib/stores/onboarding.svelte.js";
	import { dbeaverImportStore } from "$lib/stores/dbeaver-import.svelte.js";

	setDatabase();

	const db = useDatabase();
	const shortcuts = setShortcuts();
	let { children } = $props();

	// Check if we're in the theme editor window (no app shell needed)
	const isThemeEditor = $derived(page.url.pathname.startsWith("/windows/theme-editor"));

	// Initialize stores on mount
	onMount(async () => {
		await themeStore.initialize();
		await onboardingStore.initialize();
		await dbeaverImportStore.initialize();
	});

	// Apply active theme whenever it changes
	$effect(() => {
		if (themeStore.isLoaded) {
			themeStore.applyActiveTheme();
		}
	});

	function handleBeforeUnload() {
		db.persistence.flush();
		themeStore.flush();
	}

	$effect(() => {
		const unlisten = listen<string>("update-downloaded", (event) => {
			toast.success(`Update v${event.payload} downloaded`, {
				action: {
					label: "Install & Restart",
					onClick: () => invoke("install_update")
				},

				duration: Infinity
			});
		});

		return () => {
			unlisten.then((fn) => fn());
		};
	});

	// Listen for Settings menu event from Tauri
	$effect(() => {
		const unlisten = listen("menu-settings", () => {
			settingsDialogStore.open();
		});

		return () => {
			unlisten.then((fn) => fn());
		};
	});

	// Listen for theme editor color updates (real-time preview)
	$effect(() => {
		const unlisten = listen<{ colors: ThemeColors }>("theme-editor:color-update", (event) => {
			applyThemeColors(event.payload.colors);
		});

		return () => {
			unlisten.then((fn) => fn());
		};
	});

	// Listen for theme save from editor
	$effect(() => {
		const unlisten = listen<{
			themeId: string | null;
			name: string;
			isDark: boolean;
			colors: ThemeColors;
		}>("theme-editor:save", (event) => {
			const { themeId, name, isDark, colors } = event.payload;
			if (themeId) {
				themeStore.updateTheme(themeId, { name, isDark, colors });
			} else {
				themeStore.addTheme({ name, isDark, colors });
			}
			toast.success(m.theme_save_success());
		});

		return () => {
			unlisten.then((fn) => fn());
		};
	});

	// Listen for theme editor cancel (restore original theme)
	$effect(() => {
		const unlisten = listen("theme-editor:cancel", () => {
			themeStore.applyActiveTheme();
		});

		return () => {
			unlisten.then((fn) => fn());
		};
	});
</script>

<svelte:window
	onkeydown={shortcuts.handleKeydown}
	onbeforeunload={handleBeforeUnload}
></svelte:window>
<ModeWatcher />
<Toaster position="bottom-right" theme={"dark"} richColors />

{#if isThemeEditor}
	<!-- Theme editor window: minimal layout, no app shell -->
	{@render children()}
{:else}
	<!-- Main app window: full app shell with header and sidebars -->
	<KeyboardShortcutsDialog />
	<CommandPalette />
	<SettingsDialog />
	<DbeaverImportDialog />

	<Sidebar.Provider
		class="[--header-height:calc(--spacing(8))] flex-col h-svh overflow-hidden"
	>
		<AppHeader />
		<div
			class="flex w-full flex-1 min-h-0 overflow-hidden"
		>
			{@render children()}
		</div>
	</Sidebar.Provider>
	<div style="display:none">
		{#each locales as locale}
			<a
				href={localizeHref(page.url.pathname, { locale })}
			>
				{locale}
			</a>
		{/each}
	</div>
{/if}
