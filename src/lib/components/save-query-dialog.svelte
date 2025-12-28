<script lang="ts">
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "$lib/components/ui/dialog";
	import { Button } from "$lib/components/ui/button";
	import { Input } from "$lib/components/ui/input";
	import { Label } from "$lib/components/ui/label";
	import { toast } from "svelte-sonner";

	type Props = {
		open?: boolean;
		query: string;
		tabId?: string;
		onSaveComplete?: () => void;
	};

	let { open = $bindable(false), query, tabId, onSaveComplete }: Props = $props();
	const db = useDatabase();

	let queryName = $state("");

	// Pre-populate query name when dialog opens for an existing saved query
	$effect(() => {
		if (open && tabId) {
			const tab = db.queryTabs.find(t => t.id === tabId);
			if (tab?.savedQueryId) {
				const savedQuery = db.activeConnectionSavedQueries.find(q => q.id === tab.savedQueryId);
				if (savedQuery) {
					queryName = savedQuery.name;
				}
			}
		}
	});

	const handleSave = () => {
		if (!queryName.trim()) {
			toast.error("Please enter a query name");
			return;
		}

		db.saveQuery(queryName.trim(), query, tabId);
		toast.success("Query saved successfully");
		open = false;
		queryName = "";
		onSaveComplete?.();
	};

	const handleKeydown = (e: KeyboardEvent) => {
		if (e.key === "Enter") {
			handleSave();
		}
	};
</script>

<Dialog bind:open>
	<DialogContent class="max-w-md">
		<DialogHeader>
			<DialogTitle>Save Query</DialogTitle>
			<DialogDescription>Give your query a name to save it for later use</DialogDescription>
		</DialogHeader>

		<div class="grid gap-4 py-4">
			<div class="grid gap-2">
				<Label for="query-name">Query Name</Label>
				<Input id="query-name" bind:value={queryName} placeholder="e.g., Active Users Report" onkeydown={handleKeydown} />
			</div>
		</div>

		<DialogFooter>
			<Button variant="outline" onclick={() => (open = false)}>Cancel</Button>
			<Button onclick={handleSave}>Save Query</Button>
		</DialogFooter>
	</DialogContent>
</Dialog>
