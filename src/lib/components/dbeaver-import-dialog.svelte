<script lang="ts">
	import * as Dialog from "$lib/components/ui/dialog/index.js";
	import { Button } from "$lib/components/ui/button";
	import { Checkbox } from "$lib/components/ui/checkbox";
	import { dbeaverImportStore } from "$lib/stores/dbeaver-import.svelte.js";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { toast } from "svelte-sonner";
	import { m } from "$lib/paraglide/messages.js";
	import DatabaseIcon from "@lucide/svelte/icons/database";
	import AlertTriangleIcon from "@lucide/svelte/icons/alert-triangle";
	import type { DatabaseConnection } from "$lib/types";

	const db = useDatabase();

	const selectedCount = $derived(
		dbeaverImportStore.connections.filter((c) => c.selected).length
	);

	const hasSelections = $derived(selectedCount > 0);

	async function handleImport() {
		const selected = dbeaverImportStore.getSelectedConnections();
		let importedCount = 0;

		for (const conn of selected) {
			try {
				// Generate the connection ID that Seaquel would use
				const connectionId =
					conn.type === "sqlite"
						? `conn-sqlite-${conn.databaseName}`
						: `conn-${conn.host}-${conn.port}`;

				// Check if already exists
				if (db.state.connections.find((c) => c.id === connectionId)) {
					continue;
				}

				// Create connection object (without connecting - password is empty)
				const newConnection: DatabaseConnection = {
					id: connectionId,
					name: conn.name,
					type: conn.type,
					host: conn.host,
					port: conn.port,
					databaseName: conn.databaseName,
					username: conn.username,
					password: "", // User must enter this when connecting
				};

				// Add to state
				db.state.connections.push(newConnection);

				// Persist
				await db.persistence.persistConnection(newConnection);

				importedCount++;
			} catch (error) {
				console.error(`Failed to import connection ${conn.name}:`, error);
			}
		}

		if (importedCount > 0) {
			toast.success(m.dbeaver_import_success({ count: importedCount }));
		}

		await dbeaverImportStore.completeImport();
	}

	async function handleDismiss() {
		await dbeaverImportStore.dismiss();
	}
</script>

<Dialog.Root bind:open={dbeaverImportStore.isOpen}>
	<Dialog.Content class="max-w-lg">
		<Dialog.Header>
			<Dialog.Title class="flex items-center gap-2">
				<DatabaseIcon class="size-5" />
				{m.dbeaver_import_title()}
			</Dialog.Title>
			<Dialog.Description>
				{m.dbeaver_import_description()}
			</Dialog.Description>
		</Dialog.Header>

		<div class="space-y-4 py-4">
			<!-- Selection controls -->
			<div class="flex items-center justify-between text-sm">
				<span class="text-muted-foreground">
					{m.dbeaver_import_found({ count: dbeaverImportStore.connections.length })}
				</span>
				<div class="flex gap-2">
					<Button
						variant="ghost"
						size="sm"
						onclick={() => dbeaverImportStore.selectAll()}
					>
						{m.dbeaver_import_select_all()}
					</Button>
					<Button
						variant="ghost"
						size="sm"
						onclick={() => dbeaverImportStore.deselectAll()}
					>
						{m.dbeaver_import_deselect_all()}
					</Button>
				</div>
			</div>

			<!-- Connection list -->
			<div class="max-h-64 overflow-y-auto space-y-2 border rounded-lg p-2">
				{#each dbeaverImportStore.connections as conn, index}
					<label
						class="flex items-center gap-3 p-2 rounded-md hover:bg-muted/50 cursor-pointer transition-colors"
						class:opacity-50={conn.isDuplicate}
					>
						<Checkbox
							checked={conn.selected}
							disabled={conn.isDuplicate}
							onCheckedChange={() => dbeaverImportStore.toggleConnection(index)}
						/>
						<div class="flex-1 min-w-0">
							<div class="flex items-center gap-2">
								<span class="font-medium truncate">{conn.name}</span>
								<span class="text-xs text-muted-foreground uppercase"
									>{conn.type}</span
								>
							</div>
							<span class="text-xs text-muted-foreground truncate block">
								{conn.username}@{conn.host}:{conn.port}/{conn.databaseName}
							</span>
						</div>
						{#if conn.isDuplicate}
							<span class="text-xs text-amber-500 flex items-center gap-1">
								<AlertTriangleIcon class="size-3" />
								{m.dbeaver_import_duplicate()}
							</span>
						{/if}
					</label>
				{/each}
			</div>

			<!-- Password note -->
			<p class="text-xs text-muted-foreground">
				{m.dbeaver_import_password_note()}
			</p>
		</div>

		<Dialog.Footer>
			<Button variant="ghost" onclick={handleDismiss}>
				{m.dbeaver_import_skip()}
			</Button>
			<Button onclick={handleImport} disabled={!hasSelections}>
				{m.dbeaver_import_button({ count: selectedCount })}
			</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>
