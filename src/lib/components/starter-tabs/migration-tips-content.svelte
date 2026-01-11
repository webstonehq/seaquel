<script lang="ts">
	import { Button } from "$lib/components/ui/button";
	import { Card, CardContent, CardHeader, CardTitle } from "$lib/components/ui/card";
	import { onboardingStore, type UserBackground } from "$lib/stores/onboarding.svelte.js";
	import { getMigrationTrack, migrationTracks } from "$lib/config/migration-tracks.js";
	import { m } from "$lib/paraglide/messages.js";
	import { ArrowRightIcon, CheckCircleIcon, InfoIcon, KeyboardIcon } from "@lucide/svelte";

	const track = $derived(getMigrationTrack(onboardingStore.userBackground));

	const handleSelectBackground = (background: UserBackground) => {
		onboardingStore.setBackground(background);
	};
</script>

<div class="flex-1 flex items-center justify-center p-8 overflow-auto">
	<div class="max-w-2xl w-full space-y-8">
		{#if track}
			<!-- User has selected a background -->
			<div class="text-center space-y-2">
				<CheckCircleIcon class="size-12 mx-auto text-green-500" />
				<h1 class="text-2xl font-semibold">{track.welcomeMessage}</h1>
				<p class="text-muted-foreground">{track.welcomeDescription}</p>
			</div>

			<!-- Keyboard note -->
			<Card class="bg-muted/50 border-muted">
				<CardContent class="p-4 flex items-center gap-3">
					<KeyboardIcon class="size-5 text-muted-foreground shrink-0" />
					<p class="text-sm">{track.keyboardNote}</p>
				</CardContent>
			</Card>

			<!-- UI Mappings -->
			<Card>
				<CardHeader class="pb-3">
					<CardTitle class="text-base">{m.starter_ui_mapping_title()}</CardTitle>
				</CardHeader>
				<CardContent class="space-y-3">
					{#each track.uiMappings as mapping}
						<div class="flex items-center gap-3 text-sm">
							<span class="flex-1 text-muted-foreground">{mapping.theirTerm}</span>
							<ArrowRightIcon class="size-4 text-muted-foreground shrink-0" />
							<span class="flex-1 font-medium">{mapping.ourTerm}</span>
						</div>
					{/each}
				</CardContent>
			</Card>

			<!-- Change selection option -->
			<div class="text-center">
				<Button
					variant="ghost"
					size="sm"
					onclick={() => onboardingStore.setBackground("none")}
				>
					{m.starter_change_selection()}
				</Button>
			</div>
		{:else}
			<!-- User hasn't selected a background -->
			<div class="text-center space-y-2">
				<InfoIcon class="size-12 mx-auto text-muted-foreground/50" />
				<h1 class="text-2xl font-semibold">{m.starter_migration_title()}</h1>
				<p class="text-muted-foreground">{m.starter_migration_description()}</p>
			</div>

			<!-- Background selection -->
			<div class="grid gap-4 sm:grid-cols-2">
				{#each Object.values(migrationTracks) as migrationTrack}
					<Card
						class="cursor-pointer hover:bg-muted/50 transition-colors"
						onclick={() => handleSelectBackground(migrationTrack.id)}
					>
						<CardContent class="p-6 text-center space-y-2">
							<p class="text-lg font-semibold">{migrationTrack.name}</p>
							<p class="text-sm text-muted-foreground">
								{m.starter_coming_from({ tool: migrationTrack.name })}
							</p>
						</CardContent>
					</Card>
				{/each}
			</div>

			<!-- Skip option -->
			<div class="text-center">
				<p class="text-sm text-muted-foreground">
					{m.starter_migration_skip_note()}
				</p>
			</div>
		{/if}
	</div>
</div>
