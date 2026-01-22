<script lang="ts">
	import { Button } from "$lib/components/ui/button";
	import { Input } from "$lib/components/ui/input";
	import { Label } from "$lib/components/ui/label";
	import { Textarea } from "$lib/components/ui/textarea";
	import * as Dialog from "$lib/components/ui/dialog/index.js";
	import * as Select from "$lib/components/ui/select/index.js";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { toast } from "svelte-sonner";
import { errorToast } from "$lib/utils/toast";
	import type { SharedQuery } from "$lib/types";
	import { Loader2Icon, XIcon, PlusIcon } from "@lucide/svelte";

	interface Props {
		open: boolean;
		onOpenChange: (open: boolean) => void;
		query?: string;
		name?: string;
		editingQuery?: SharedQuery | null;
	}

	let { open = $bindable(), onOpenChange, query: initialQuery = "", name: initialName = "", editingQuery = null }: Props = $props();

	const db = useDatabase();

	let name = $state("");
	let description = $state("");
	let queryText = $state("");
	let folder = $state("");
	let databaseType = $state("");
	let tags = $state<string[]>([]);
	let newTag = $state("");
	let isLoading = $state(false);

	// Reset form when dialog opens
	$effect(() => {
		if (open) {
			if (editingQuery) {
				name = editingQuery.name;
				description = editingQuery.description ?? "";
				queryText = editingQuery.query;
				folder = editingQuery.folder;
				databaseType = editingQuery.databaseType ?? "";
				tags = [...editingQuery.tags];
			} else {
				name = initialName;
				description = "";
				queryText = initialQuery;
				folder = "";
				databaseType = "";
				tags = [];
			}
		}
	});

	const folders = $derived.by(() => {
		if (!db.state.activeRepoId) return [];
		return db.sharedQueries.getFolders(db.state.activeRepoId);
	});

	async function handleSubmit() {
		if (!name.trim()) {
			errorToast("Please enter a name for the query");
			return;
		}

		if (!queryText.trim()) {
			errorToast("Please enter the SQL query");
			return;
		}

		if (!db.state.activeRepoId) {
			errorToast("Please select a repository first");
			return;
		}

		isLoading = true;

		try {
			if (editingQuery) {
				// Update existing query
				await db.sharedQueries.updateQuery(editingQuery.id, {
					name: name.trim(),
					description: description.trim() || undefined,
					query: queryText.trim(),
					databaseType: databaseType || undefined,
					tags
				});
				toast.success(`Query "${name}" updated`);
			} else {
				// Create new query
				await db.sharedQueries.createQuery(name.trim(), queryText.trim(), folder, {
					description: description.trim() || undefined,
					databaseType: databaseType || undefined,
					tags
				});
				toast.success(`Query "${name}" saved to shared repository`);
			}

			onOpenChange(false);
		} catch (error) {
			const message = error instanceof Error ? error.message : String(error);
			errorToast(`Failed to save query: ${message}`);
		} finally {
			isLoading = false;
		}
	}

	function addTag() {
		const tag = newTag.trim().toLowerCase();
		if (tag && !tags.includes(tag)) {
			tags = [...tags, tag];
		}
		newTag = "";
	}

	function removeTag(tag: string) {
		tags = tags.filter((t) => t !== tag);
	}

	function handleTagKeydown(e: KeyboardEvent) {
		if (e.key === "Enter") {
			e.preventDefault();
			addTag();
		}
	}
</script>

<Dialog.Root {open} {onOpenChange}>
	<Dialog.Content class="max-w-2xl max-h-[90vh] overflow-y-auto">
		<Dialog.Header>
			<Dialog.Title>
				{editingQuery ? "Edit Shared Query" : "Share Query"}
			</Dialog.Title>
			<Dialog.Description>
				{editingQuery
					? "Update the query details and save to the shared repository."
					: "Save this query to the shared repository for your team to use."}
			</Dialog.Description>
		</Dialog.Header>

		<div class="space-y-4 py-4">
			<div class="grid grid-cols-2 gap-4">
				<div class="space-y-2">
					<Label for="name">Name</Label>
					<Input
						id="name"
						bind:value={name}
						placeholder="Get active users"
						disabled={isLoading}
					/>
				</div>

				<div class="space-y-2">
					<Label for="folder">Folder</Label>
					{#if folders.length > 0}
						<Select.Root type="single" bind:value={folder}>
							<Select.Trigger class="w-full">
								{folder || "Root (no folder)"}
							</Select.Trigger>
							<Select.Content>
								<Select.Item value="">Root (no folder)</Select.Item>
								{#each folders as f (f)}
									<Select.Item value={f}>{f}</Select.Item>
								{/each}
							</Select.Content>
						</Select.Root>
					{:else}
						<Input
							id="folder"
							bind:value={folder}
							placeholder="analytics"
							disabled={isLoading}
						/>
					{/if}
				</div>
			</div>

			<div class="space-y-2">
				<Label for="description">Description</Label>
				<Input
					id="description"
					bind:value={description}
					placeholder="Returns users who logged in within the last 30 days"
					disabled={isLoading}
				/>
			</div>

			<div class="space-y-2">
				<Label for="query">SQL Query</Label>
				<Textarea
					id="query"
					bind:value={queryText}
					placeholder="SELECT * FROM users WHERE ..."
					class="font-mono text-sm min-h-[150px]"
					disabled={isLoading}
				/>
			</div>

			<div class="grid grid-cols-2 gap-4">
				<div class="space-y-2">
					<Label for="database-type">Database Type</Label>
					<Select.Root type="single" bind:value={databaseType}>
						<Select.Trigger class="w-full">
							{databaseType || "Any"}
						</Select.Trigger>
						<Select.Content>
							<Select.Item value="">Any</Select.Item>
							<Select.Item value="postgresql">PostgreSQL</Select.Item>
							<Select.Item value="mysql">MySQL</Select.Item>
							<Select.Item value="mssql">SQL Server</Select.Item>
							<Select.Item value="sqlite">SQLite</Select.Item>
							<Select.Item value="duckdb">DuckDB</Select.Item>
						</Select.Content>
					</Select.Root>
				</div>

				<div class="space-y-2">
					<Label for="tags">Tags</Label>
					<div class="flex gap-2">
						<Input
							id="tags"
							bind:value={newTag}
							placeholder="Add tag..."
							disabled={isLoading}
							class="flex-1"
							onkeydown={handleTagKeydown}
						/>
						<Button
							variant="outline"
							size="icon"
							onclick={addTag}
							disabled={isLoading || !newTag.trim()}
						>
							<PlusIcon class="size-4" />
						</Button>
					</div>
					{#if tags.length > 0}
						<div class="flex flex-wrap gap-1 mt-2">
							{#each tags as tag (tag)}
								<span class="inline-flex items-center gap-1 px-2 py-0.5 text-xs bg-muted rounded-md">
									{tag}
									<button
										type="button"
										class="hover:text-destructive"
										onclick={() => removeTag(tag)}
									>
										<XIcon class="size-3" />
									</button>
								</span>
							{/each}
						</div>
					{/if}
				</div>
			</div>
		</div>

		<Dialog.Footer>
			<Button variant="outline" onclick={() => onOpenChange(false)} disabled={isLoading}>
				Cancel
			</Button>
			<Button onclick={handleSubmit} disabled={isLoading}>
				{#if isLoading}
					<Loader2Icon class="size-4 me-2 animate-spin" />
				{/if}
				{editingQuery ? "Update Query" : "Share Query"}
			</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>
