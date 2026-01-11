<script lang="ts">
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { Badge } from "$lib/components/ui/badge";
	import { Button } from "$lib/components/ui/button";
	import { Input } from "$lib/components/ui/input";
	import * as Popover from "$lib/components/ui/popover/index.js";
	import { CheckIcon, PlusIcon, XIcon, TagIcon } from "@lucide/svelte";
	import { m } from "$lib/paraglide/messages.js";
	import type { ConnectionLabel } from "$lib/types";

	type Props = {
		connectionId: string;
		compact?: boolean;
	};

	let { connectionId, compact = false }: Props = $props();

	const db = useDatabase();

	// Get available labels for the current project
	const availableLabels = $derived(
		db.state.activeProjectId
			? db.labels.getLabelsForProject(db.state.activeProjectId)
			: []
	);

	// Get current labels for this connection
	const connectionLabels = $derived(db.labels.getConnectionLabelsById(connectionId));
	const connectionLabelIds = $derived(new Set(connectionLabels.map(l => l.id)));

	// State for adding custom label
	let showCustomInput = $state(false);
	let customLabelName = $state("");
	let popoverOpen = $state(false);

	// Default color for new labels
	let selectedColor = $state("#6366f1");

	const toggleLabel = (labelId: string) => {
		if (connectionLabelIds.has(labelId)) {
			db.labels.removeLabelFromConnection(connectionId, labelId);
		} else {
			db.labels.addLabelToConnection(connectionId, labelId);
		}
	};

	const addCustomLabel = () => {
		if (!customLabelName.trim() || !db.state.activeProjectId) return;

		// Create custom label and add to connection
		const newLabel = db.projects.addCustomLabel(db.state.activeProjectId, {
			name: customLabelName.trim(),
			color: selectedColor
		});
		if (newLabel) {
			db.labels.addLabelToConnection(connectionId, newLabel.id);
		}

		// Reset state
		customLabelName = "";
		showCustomInput = false;
		selectedColor = "#6366f1";
	};

	const removeLabel = (labelId: string) => {
		db.labels.removeLabelFromConnection(connectionId, labelId);
	};
</script>

{#if compact}
	<!-- Compact mode: just show badges with a + button -->
	<div class="flex items-center gap-1 flex-wrap">
		{#each connectionLabels as label (label.id)}
			<Badge
				variant="outline"
				class="text-[10px] px-1.5 py-0 h-5 gap-1 group cursor-default"
				style="border-color: {label.color}; color: {label.color};"
			>
				{label.name}
				<button
					class="opacity-0 group-hover:opacity-100 transition-opacity hover:text-destructive"
					onclick={(e) => { e.stopPropagation(); removeLabel(label.id); }}
				>
					<XIcon class="size-3" />
				</button>
			</Badge>
		{/each}
		<Popover.Root bind:open={popoverOpen}>
			<Popover.Trigger>
				<Button size="icon" variant="ghost" class="size-5 [&_svg]:size-3">
					<TagIcon />
				</Button>
			</Popover.Trigger>
			<Popover.Content class="w-64 p-2" align="start">
				<div class="space-y-2">
					<p class="text-xs font-medium text-muted-foreground px-1">{m.labels_select()}</p>
					<div class="space-y-1">
						{#each availableLabels as label (label.id)}
							<button
								class="w-full flex items-center gap-2 px-2 py-1.5 rounded-md hover:bg-muted text-sm transition-colors"
								onclick={() => toggleLabel(label.id)}
							>
								<span class="w-4">
									{#if connectionLabelIds.has(label.id)}
										<CheckIcon class="size-4" style="color: {label.color};" />
									{/if}
								</span>
								<span
									class="size-3 rounded-full"
									style="background-color: {label.color};"
								></span>
								<span class="flex-1 text-left">{label.name}</span>
							</button>
						{/each}
					</div>

					{#if showCustomInput}
						<div class="pt-2 border-t space-y-2">
							<Input
								bind:value={customLabelName}
								placeholder={m.labels_custom_placeholder()}
								class="h-8 text-sm"
								onkeydown={(e) => e.key === "Enter" && addCustomLabel()}
							/>
							<label class="relative size-6 cursor-pointer">
								<input
									type="color"
									bind:value={selectedColor}
									class="absolute inset-0 opacity-0 cursor-pointer"
								/>
								<span
									class="block size-6 rounded-full border-2 border-foreground"
									style="background-color: {selectedColor};"
								></span>
							</label>
							<div class="flex gap-2">
								<Button size="sm" variant="outline" class="flex-1 h-7" onclick={() => showCustomInput = false}>
									{m.labels_cancel()}
								</Button>
								<Button size="sm" class="flex-1 h-7" onclick={addCustomLabel} disabled={!customLabelName.trim()}>
									{m.labels_add()}
								</Button>
							</div>
						</div>
					{:else}
						<button
							class="w-full flex items-center gap-2 px-2 py-1.5 rounded-md hover:bg-muted text-sm text-muted-foreground transition-colors"
							onclick={() => showCustomInput = true}
						>
							<PlusIcon class="size-4" />
							{m.labels_create_custom()}
						</button>
					{/if}
				</div>
			</Popover.Content>
		</Popover.Root>
	</div>
{:else}
	<!-- Full mode: show in a form-like layout -->
	<div class="space-y-3">
		<div class="flex items-center gap-2 flex-wrap">
			{#each connectionLabels as label (label.id)}
				<Badge
					variant="outline"
					class="text-xs px-2 py-0.5 h-6 gap-1 group cursor-default"
					style="border-color: {label.color}; color: {label.color};"
				>
					{label.name}
					<button
						class="opacity-0 group-hover:opacity-100 transition-opacity hover:text-destructive"
						onclick={() => removeLabel(label.id)}
					>
						<XIcon class="size-3" />
					</button>
				</Badge>
			{/each}
			{#if connectionLabels.length === 0}
				<span class="text-sm text-muted-foreground">{m.labels_none()}</span>
			{/if}
		</div>

		<div class="space-y-2">
			<p class="text-xs font-medium text-muted-foreground">{m.labels_available()}</p>
			<div class="flex flex-wrap gap-2">
				{#each availableLabels.filter(l => !connectionLabelIds.has(l.id)) as label (label.id)}
					<button
						class="flex items-center gap-1.5 px-2 py-1 rounded-md border text-sm hover:bg-muted transition-colors"
						onclick={() => toggleLabel(label.id)}
					>
						<span
							class="size-2.5 rounded-full"
							style="background-color: {label.color};"
						></span>
						{label.name}
					</button>
				{/each}

				{#if showCustomInput}
					<div class="flex items-center gap-2 w-full mt-2">
						<Input
							bind:value={customLabelName}
							placeholder={m.labels_custom_placeholder()}
							class="h-8 text-sm flex-1"
							onkeydown={(e) => e.key === "Enter" && addCustomLabel()}
						/>
						<label class="relative size-6 cursor-pointer">
							<input
								type="color"
								bind:value={selectedColor}
								class="absolute inset-0 opacity-0 cursor-pointer"
							/>
							<span
								class="block size-6 rounded-full border-2 border-foreground"
								style="background-color: {selectedColor};"
							></span>
						</label>
						<Button size="sm" variant="ghost" class="h-8" onclick={() => showCustomInput = false}>
							<XIcon class="size-4" />
						</Button>
						<Button size="sm" class="h-8" onclick={addCustomLabel} disabled={!customLabelName.trim()}>
							{m.labels_add()}
						</Button>
					</div>
				{:else}
					<button
						class="flex items-center gap-1.5 px-2 py-1 rounded-md border border-dashed text-sm text-muted-foreground hover:bg-muted transition-colors"
						onclick={() => showCustomInput = true}
					>
						<PlusIcon class="size-3" />
						{m.labels_create_custom()}
					</button>
				{/if}
			</div>
		</div>
	</div>
{/if}
