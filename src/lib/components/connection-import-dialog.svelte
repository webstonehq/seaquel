<script lang="ts">
	import * as Dialog from "$lib/components/ui/dialog/index.js";
	import { Button } from "$lib/components/ui/button";
	import { Checkbox } from "$lib/components/ui/checkbox";
	import type { ConnectionImportStore } from "$lib/stores/connection-import.svelte.js";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { toast } from "svelte-sonner";
	import { errorToast } from "$lib/utils/toast";
	import { extractErrorMessage } from "$lib/errors";
	import { m } from "$lib/paraglide/messages.js";
	import DatabaseIcon from "@lucide/svelte/icons/database";
	import AlertTriangleIcon from "@lucide/svelte/icons/alert-triangle";

	interface Props {
		store: ConnectionImportStore;
	}

	let { store }: Props = $props();

	const db = useDatabase();

	/** The dialog's words for each tool. */
	const text = $derived(
		store.source === "tableplus"
			? {
					title: m.tableplus_import_title(),
					description: m.tableplus_import_description(),
					found: m.tableplus_import_found,
					selectAll: m.tableplus_import_select_all(),
					deselectAll: m.tableplus_import_deselect_all(),
					duplicate: m.tableplus_import_duplicate(),
					passwordNote: m.tableplus_import_password_note(),
					skip: m.tableplus_import_skip(),
					button: m.tableplus_import_button,
					success: m.tableplus_import_success,
				}
			: {
					title: m.dbeaver_import_title(),
					description: m.dbeaver_import_description(),
					found: m.dbeaver_import_found,
					selectAll: m.dbeaver_import_select_all(),
					deselectAll: m.dbeaver_import_deselect_all(),
					duplicate: m.dbeaver_import_duplicate(),
					passwordNote: m.dbeaver_import_password_note(),
					skip: m.dbeaver_import_skip(),
					button: m.dbeaver_import_button,
					success: m.dbeaver_import_success,
				},
	);

	const selectedCount = $derived(store.candidates.filter((c) => c.selected).length);

	let importing = $state(false);

	async function handleImport() {
		const projectId = store.projectId;
		if (!projectId) return;
		importing = true;
		try {
			// Core reads the file again and imports the ticked keys in one go.
			const { imported, failures } = await db.connections.importConnections(
				store.source,
				projectId,
				store.selected(),
			);
			if (imported > 0) toast.success(text.success({ count: imported }));
			if (failures.length > 0) {
				errorToast(
					m.import_connections_failed_reasons({
						count: failures.length,
						failures: failures.map((f) => `${f.name} (${f.reason})`).join(", "),
					}),
				);
			}
			await store.completeImport();
		} catch (error) {
			errorToast(m.import_connections_error({ message: extractErrorMessage(error) }));
		} finally {
			importing = false;
		}
	}
</script>

<Dialog.Root bind:open={store.isOpen}>
	<Dialog.Content class="max-w-lg">
		<Dialog.Header>
			<Dialog.Title class="flex items-center gap-2">
				<DatabaseIcon class="size-5" />
				{text.title}
			</Dialog.Title>
			<Dialog.Description>
				{text.description}
			</Dialog.Description>
		</Dialog.Header>

		<div class="space-y-4 py-4">
			<!-- Selection controls -->
			<div class="flex items-center justify-between text-sm">
				<span class="text-muted-foreground">
					{text.found({ count: store.candidates.length })}
				</span>
				<div class="flex gap-2">
					<Button variant="ghost" size="sm" onclick={() => store.selectAll()}>
						{text.selectAll}
					</Button>
					<Button variant="ghost" size="sm" onclick={() => store.deselectAll()}>
						{text.deselectAll}
					</Button>
				</div>
			</div>

			<!-- Connection list -->
			<div class="max-h-64 overflow-y-auto space-y-2 border rounded-lg p-2">
				{#each store.candidates as conn, index (conn.key)}
					{@const problem = store.problemText(conn)}
					<label
						class="flex items-start gap-3 p-2 rounded-md hover:bg-muted/50 cursor-pointer transition-colors"
						class:opacity-50={!store.canSelect(index)}
					>
						<Checkbox
							checked={conn.selected}
							disabled={!store.canSelect(index)}
							onCheckedChange={() => store.toggleConnection(index)}
						/>
						<div class="flex-1 min-w-0">
							<div class="flex items-center gap-2">
								<span class="font-medium">{conn.name}</span>
								<span class="text-xs text-muted-foreground uppercase shrink-0">{conn.type}</span>
							</div>
							<p class="text-xs text-muted-foreground break-all">
								{conn.username}@{conn.host}:{conn.port}/{conn.databaseName}
							</p>
						</div>
						{#if problem}
							<span class="text-xs text-destructive flex items-center gap-1 shrink-0">
								<AlertTriangleIcon class="size-3" />
								{problem}
							</span>
						{:else if conn.duplicateOf}
							<span class="text-xs text-amber-500 flex items-center gap-1 shrink-0">
								<AlertTriangleIcon class="size-3" />
								{text.duplicate}
							</span>
						{/if}
					</label>
				{/each}
			</div>

			<!-- Password note -->
			<p class="text-xs text-muted-foreground">
				{text.passwordNote}
			</p>
		</div>

		<Dialog.Footer>
			<Button variant="ghost" onclick={() => store.dismiss()}>
				{text.skip}
			</Button>
			<Button onclick={handleImport} disabled={selectedCount === 0 || importing}>
				{text.button({ count: selectedCount })}
			</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>
