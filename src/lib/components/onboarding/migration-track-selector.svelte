<script lang="ts">
	import { Button } from "$lib/components/ui/button";
	import { m } from "$lib/paraglide/messages.js";
	import { onboardingStore, type UserBackground } from "$lib/stores/onboarding.svelte.js";
	import { dbeaverImportStore } from "$lib/stores/dbeaver-import.svelte.js";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { migrationTracks, getMigrationTrack } from "$lib/config/migration-tracks.js";
	import DatabaseIcon from "@lucide/svelte/icons/database";
	import CheckIcon from "@lucide/svelte/icons/check";
	import ArrowRightIcon from "@lucide/svelte/icons/arrow-right";

	interface Props {
		onSelect?: (background: UserBackground) => void;
	}

	let { onSelect }: Props = $props();

	const db = useDatabase();

	let selectedBackground = $state<UserBackground>("none");
	let showConfirmation = $state(false);

	// Sync from store on mount
	$effect.pre(() => {
		selectedBackground = onboardingStore.userBackground;
		showConfirmation = onboardingStore.userBackground !== "none";
	});

	const handleSelect = async (background: UserBackground) => {
		selectedBackground = background;
		onboardingStore.setBackground(background);
		showConfirmation = background !== "none";
		onSelect?.(background);

		// Show DBeaver import dialog when user clicks DBeaver card
		if (background === "dbeaver") {
			const existingIds = db.state.connections.map((c) => c.id);
			await dbeaverImportStore.checkAndShowDialog(existingIds);
		}
	};

	const handleChangeSelection = () => {
		showConfirmation = false;
	};

	const selectedTrack = $derived(getMigrationTrack(selectedBackground));
	const tracks = Object.values(migrationTracks);
</script>

{#if showConfirmation && selectedTrack}
	<!-- Confirmation view with track-specific info -->
	<div class="space-y-4 animate-in fade-in slide-in-from-bottom-2 duration-300">
		<div class="text-center space-y-2">
			<div class="size-12 rounded-full bg-primary/10 flex items-center justify-center mx-auto">
				<CheckIcon class="size-6 text-primary" />
			</div>
			<h3 class="font-semibold">{selectedTrack.welcomeMessage}</h3>
			<p class="text-sm text-muted-foreground">{selectedTrack.welcomeDescription}</p>
		</div>

		<div class="rounded-lg border bg-muted/30 p-4 space-y-3">
			<p class="text-xs font-medium text-muted-foreground uppercase tracking-wide">
				{m.migration_ui_mapping()}
			</p>
			<div class="space-y-2">
				{#each selectedTrack.uiMappings as mapping}
					<div class="flex items-center gap-2 text-sm">
						<span class="text-muted-foreground">{mapping.theirTerm}</span>
						<ArrowRightIcon class="size-3 text-muted-foreground shrink-0" />
						<span class="font-medium">{mapping.ourTerm}</span>
					</div>
				{/each}
			</div>
		</div>

		<p class="text-xs text-center text-muted-foreground">{selectedTrack.keyboardNote}</p>

		<Button variant="ghost" size="sm" class="w-full" onclick={handleChangeSelection}>
			{m.migration_change()}
		</Button>
	</div>
{:else}
	<!-- Selection view -->
	<div class="space-y-4">
		<div class="text-center space-y-1">
			<h3 class="font-medium">{m.migration_title()}</h3>
			<p class="text-xs text-muted-foreground">{m.migration_description()}</p>
		</div>

		<div class="grid grid-cols-2 gap-3">
			{#each tracks as track}
				<button
					type="button"
					class="relative flex flex-col items-center gap-2 p-4 rounded-lg border transition-all hover:bg-accent/50 focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2 {selectedBackground ===
					track.id
						? 'border-primary bg-primary/5'
						: 'border-border'}"
					onclick={() => handleSelect(track.id)}
				>
					{#if selectedBackground === track.id}
						<div class="absolute top-2 right-2">
							<div class="size-4 rounded-full bg-primary flex items-center justify-center">
								<CheckIcon class="size-2.5 text-primary-foreground" />
							</div>
						</div>
					{/if}
					<div class="size-10 rounded-lg bg-muted flex items-center justify-center">
						<DatabaseIcon class="size-5 text-muted-foreground" />
					</div>
					<span class="text-sm font-medium">{track.name}</span>
				</button>
			{/each}
		</div>
	</div>
{/if}
