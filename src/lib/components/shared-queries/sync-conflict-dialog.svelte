<script lang="ts">
	import { Button } from "$lib/components/ui/button";
	import * as Dialog from "$lib/components/ui/dialog/index.js";
	import * as Tabs from "$lib/components/ui/tabs/index.js";
	import { ScrollArea } from "$lib/components/ui/scroll-area";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { toast } from "svelte-sonner";
import { errorToast } from "$lib/utils/toast";
	import { resolveConflict, getConflictContent } from "$lib/services/git.js";
	import type { ConflictContent } from "$lib/types";
	import {
		Loader2Icon,
		AlertTriangleIcon,
		CheckIcon,
		ChevronRightIcon,
		FileTextIcon
	} from "@lucide/svelte";

	interface Props {
		open: boolean;
		onOpenChange: (open: boolean) => void;
		repoId: string;
		conflictFiles: string[];
	}

	let { open = $bindable(), onOpenChange, repoId, conflictFiles }: Props = $props();

	const db = useDatabase();

	let currentFileIndex = $state(0);
	let currentContent = $state<ConflictContent | null>(null);
	let isLoading = $state(false);
	let isResolving = $state(false);
	let resolvedFiles = $state<Set<string>>(new Set());

	const repo = $derived(db.state.sharedRepos.find((r) => r.id === repoId));
	const currentFile = $derived(conflictFiles[currentFileIndex] ?? null);
	const allResolved = $derived(resolvedFiles.size === conflictFiles.length);

	// Load conflict content when current file changes
	$effect(() => {
		if (open && currentFile && repo) {
			loadConflictContent();
		}
	});

	async function loadConflictContent() {
		if (!repo || !currentFile) return;

		isLoading = true;
		try {
			currentContent = await getConflictContent(repo.path, currentFile);
		} catch (error) {
			const message = error instanceof Error ? error.message : String(error);
			errorToast(`Failed to load conflict: ${message}`);
			currentContent = null;
		} finally {
			isLoading = false;
		}
	}

	async function handleResolve(resolution: "ours" | "theirs" | "base") {
		if (!repo || !currentFile || !currentContent) return;

		isResolving = true;
		try {
			let content: string;
			switch (resolution) {
				case "ours":
					content = currentContent.ours ?? "";
					break;
				case "theirs":
					content = currentContent.theirs ?? "";
					break;
				case "base":
					content = currentContent.base ?? "";
					break;
			}

			await resolveConflict(repo.path, currentFile, content);
			resolvedFiles = new Set([...resolvedFiles, currentFile]);

			// Move to next unresolved file
			const nextUnresolved = conflictFiles.findIndex(
				(f, i) => i > currentFileIndex && !resolvedFiles.has(f)
			);
			if (nextUnresolved !== -1) {
				currentFileIndex = nextUnresolved;
			}

			toast.success(`Resolved ${currentFile}`);
		} catch (error) {
			const message = error instanceof Error ? error.message : String(error);
			errorToast(`Failed to resolve conflict: ${message}`);
		} finally {
			isResolving = false;
		}
	}

	async function handleFinish() {
		if (!repo) return;

		try {
			await db.sharedRepos.commitChanges(repoId, "Resolved merge conflicts");
			await db.sharedRepos.refreshRepoStatus(repoId);
			toast.success("Conflicts resolved and committed");
			onOpenChange(false);
		} catch (error) {
			const message = error instanceof Error ? error.message : String(error);
			errorToast(`Failed to commit resolution: ${message}`);
		}
	}

	function selectFile(index: number) {
		currentFileIndex = index;
	}
</script>

<Dialog.Root {open} {onOpenChange}>
	<Dialog.Content class="max-w-4xl max-h-[85vh] flex flex-col">
		<Dialog.Header>
			<Dialog.Title class="flex items-center gap-2">
				<AlertTriangleIcon class="size-5 text-orange-500" />
				Merge Conflicts
			</Dialog.Title>
			<Dialog.Description>
				{conflictFiles.length} file{conflictFiles.length === 1 ? " has" : "s have"} conflicts that need to be resolved.
				Choose which version to keep for each file.
			</Dialog.Description>
		</Dialog.Header>

		<div class="flex flex-1 gap-4 min-h-0 py-4">
			<!-- File list sidebar -->
			<div class="w-48 shrink-0 border-r pr-4">
				<div class="text-sm font-medium mb-2 text-muted-foreground">Files</div>
				<ScrollArea class="h-[400px]">
					<div class="space-y-1">
						{#each conflictFiles as file, index (file)}
							<button
								type="button"
								class={[
									"w-full text-left px-2 py-1.5 rounded-md text-sm flex items-center gap-2 transition-colors",
									index === currentFileIndex
										? "bg-accent text-accent-foreground"
										: "hover:bg-muted"
								]}
								onclick={() => selectFile(index)}
							>
								{#if resolvedFiles.has(file)}
									<CheckIcon class="size-3.5 text-green-500 shrink-0" />
								{:else}
									<FileTextIcon class="size-3.5 text-muted-foreground shrink-0" />
								{/if}
								<span class="truncate flex-1">{file.split("/").pop()}</span>
								{#if index === currentFileIndex}
									<ChevronRightIcon class="size-3.5 shrink-0" />
								{/if}
							</button>
						{/each}
					</div>
				</ScrollArea>
			</div>

			<!-- Content comparison -->
			<div class="flex-1 min-w-0">
				{#if isLoading}
					<div class="flex items-center justify-center h-full">
						<Loader2Icon class="size-8 animate-spin text-muted-foreground" />
					</div>
				{:else if currentContent}
					<Tabs.Root value="ours" class="h-full flex flex-col">
						<Tabs.List class="grid grid-cols-3 w-full">
							<Tabs.Trigger value="ours" class="text-xs">
								Your Version (Local)
							</Tabs.Trigger>
							<Tabs.Trigger value="theirs" class="text-xs">
								Their Version (Remote)
							</Tabs.Trigger>
							<Tabs.Trigger value="base" class="text-xs" disabled={!currentContent.base}>
								Base Version
							</Tabs.Trigger>
						</Tabs.List>

						<div class="flex-1 min-h-0 mt-2">
							<Tabs.Content value="ours" class="h-full">
								<ScrollArea class="h-[350px] border rounded-md">
									<pre class="p-3 text-xs font-mono whitespace-pre-wrap">{currentContent.ours ?? "(No local version)"}</pre>
								</ScrollArea>
							</Tabs.Content>

							<Tabs.Content value="theirs" class="h-full">
								<ScrollArea class="h-[350px] border rounded-md">
									<pre class="p-3 text-xs font-mono whitespace-pre-wrap">{currentContent.theirs ?? "(No remote version)"}</pre>
								</ScrollArea>
							</Tabs.Content>

							<Tabs.Content value="base" class="h-full">
								<ScrollArea class="h-[350px] border rounded-md">
									<pre class="p-3 text-xs font-mono whitespace-pre-wrap">{currentContent.base ?? "(No base version)"}</pre>
								</ScrollArea>
							</Tabs.Content>
						</div>
					</Tabs.Root>

					{#if !resolvedFiles.has(currentFile ?? "")}
						<div class="flex gap-2 mt-4">
							<Button
								variant="outline"
								class="flex-1"
								onclick={() => handleResolve("ours")}
								disabled={isResolving}
							>
								Keep Your Version
							</Button>
							<Button
								variant="outline"
								class="flex-1"
								onclick={() => handleResolve("theirs")}
								disabled={isResolving}
							>
								Keep Their Version
							</Button>
							{#if currentContent.base}
								<Button
									variant="outline"
									class="flex-1"
									onclick={() => handleResolve("base")}
									disabled={isResolving}
								>
									Keep Base Version
								</Button>
							{/if}
						</div>
					{:else}
						<div class="flex items-center justify-center gap-2 mt-4 py-2 text-sm text-green-600">
							<CheckIcon class="size-4" />
							This file has been resolved
						</div>
					{/if}
				{:else}
					<div class="flex items-center justify-center h-full text-muted-foreground">
						Select a file to view its conflict
					</div>
				{/if}
			</div>
		</div>

		<Dialog.Footer>
			<div class="flex items-center gap-2 mr-auto text-sm text-muted-foreground">
				{resolvedFiles.size} of {conflictFiles.length} resolved
			</div>
			<Button variant="outline" onclick={() => onOpenChange(false)}>
				Cancel
			</Button>
			<Button onclick={handleFinish} disabled={!allResolved}>
				{#if isResolving}
					<Loader2Icon class="size-4 me-2 animate-spin" />
				{/if}
				Finish & Commit
			</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>
