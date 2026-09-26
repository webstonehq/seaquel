<script lang="ts">
	import type { DestructiveReason, DestructiveStatement } from "$lib/sql";
	import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";
	import * as m from "$lib/paraglide/messages.js";

	type Props = {
		open?: boolean;
		statements: DestructiveStatement[];
		onconfirm: () => void;
	};

	let { open = $bindable(false), statements, onconfirm }: Props = $props();

	const reasonLabel: Record<DestructiveReason, () => string> = {
		drop_table: m.destructive_reason_drop_table,
		drop_index: m.destructive_reason_drop_index,
		drop_view: m.destructive_reason_drop_view,
		drop_schema: m.destructive_reason_drop_schema,
		drop_database: m.destructive_reason_drop_database,
		drop_column: m.destructive_reason_drop_column,
		truncate: m.destructive_reason_truncate,
		delete_no_where: m.destructive_reason_delete_no_where,
		update_no_where: m.destructive_reason_update_no_where,
		drop_sequence: m.destructive_reason_drop_sequence,
		drop_function: m.destructive_reason_drop_function,
		merge_delete: m.destructive_reason_merge_delete,
	};

	function truncateSql(sql: string, maxLength = 80): string {
		const oneLine = sql.replace(/\s+/g, " ").trim();
		if (oneLine.length <= maxLength) return oneLine;
		return oneLine.slice(0, maxLength) + "…";
	}
</script>

<AlertDialog.Root bind:open>
	<AlertDialog.Content>
		<AlertDialog.Header>
			<AlertDialog.Title>{m.destructive_query_confirm_title()}</AlertDialog.Title>
			<AlertDialog.Description>
				{m.destructive_query_confirm_description()}
			</AlertDialog.Description>
		</AlertDialog.Header>
		<div class="flex max-h-48 flex-col gap-2 overflow-y-auto py-2">
			{#each statements as stmt (stmt.index)}
				<div class="bg-muted rounded-md px-3 py-2 text-sm">
					<span class="text-destructive font-medium">{reasonLabel[stmt.reason]()}</span>
					<code class="text-muted-foreground mt-1 block truncate text-xs">{truncateSql(stmt.sql)}</code>
				</div>
			{/each}
		</div>
		<AlertDialog.Footer>
			<AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
			<AlertDialog.Action
				onclick={onconfirm}
				class="bg-destructive text-white hover:bg-destructive/90"
			>
				{m.destructive_query_confirm_button()}
			</AlertDialog.Action>
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>
