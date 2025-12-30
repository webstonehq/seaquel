<script lang="ts">
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "$lib/components/ui/dialog";
	import { Button } from "$lib/components/ui/button";
	import { Input } from "$lib/components/ui/input";
	import { Label } from "$lib/components/ui/label";
	import { toast } from "svelte-sonner";
	import { m } from "$lib/paraglide/messages.js";

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
			const tab = db.state.queryTabs.find(t => t.id === tabId);
			if (tab?.savedQueryId) {
				const savedQuery = db.state.activeConnectionSavedQueries.find(q => q.id === tab.savedQueryId);
				if (savedQuery) {
					queryName = savedQuery.name;
				}
			}
		}
	});

	const handleSave = () => {
		if (!queryName.trim()) {
			toast.error(m.save_query_error_name());
			return;
		}

		db.savedQueries.saveQuery(queryName.trim(), query, tabId);
		toast.success(m.save_query_success());
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
			<DialogTitle>{m.save_query_title()}</DialogTitle>
			<DialogDescription>{m.save_query_description()}</DialogDescription>
		</DialogHeader>

		<div class="grid gap-4 py-4">
			<div class="grid gap-2">
				<Label for="query-name">{m.save_query_label()}</Label>
				<Input id="query-name" bind:value={queryName} placeholder={m.save_query_placeholder()} onkeydown={handleKeydown} />
			</div>
		</div>

		<DialogFooter>
			<Button variant="outline" onclick={() => (open = false)}>{m.save_query_cancel()}</Button>
			<Button onclick={handleSave}>{m.save_query_save()}</Button>
		</DialogFooter>
	</DialogContent>
</Dialog>
