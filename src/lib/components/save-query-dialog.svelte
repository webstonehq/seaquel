<script lang="ts">
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import {
		Dialog,
		DialogContent,
		DialogDescription,
		DialogFooter,
		DialogHeader,
		DialogTitle
	} from "$lib/components/ui/dialog";
	import { Button } from "$lib/components/ui/button";
	import { Input } from "$lib/components/ui/input";
	import { Label } from "$lib/components/ui/label";
	import {
		Select,
		SelectContent,
		SelectItem,
		SelectTrigger
	} from "$lib/components/ui/select";
	import {
		Collapsible,
		CollapsibleContent,
		CollapsibleTrigger
	} from "$lib/components/ui/collapsible";
	import { toast } from "svelte-sonner";
	import { m } from "$lib/paraglide/messages.js";
	import { extractParameters } from "$lib/db/query-params.js";
	import type { QueryParameter, QueryParameterType } from "$lib/types";
	import ChevronDownIcon from "@lucide/svelte/icons/chevron-down";

	type Props = {
		open?: boolean;
		query: string;
		tabId?: string;
		onSaveComplete?: () => void;
	};

	let { open = $bindable(false), query, tabId, onSaveComplete }: Props = $props();
	const db = useDatabase();

	let queryName = $state("");
	let paramsOpen = $state(false);
	let parameterConfigs = $state<QueryParameter[]>([]);

	// Extract parameters and initialize configs when dialog opens
	$effect(() => {
		if (open) {
			// Get existing saved query if linked
			const tab = tabId ? db.state.queryTabs.find((t) => t.id === tabId) : null;
			const savedQuery = tab?.savedQueryId
				? db.state.activeConnectionSavedQueries.find((q) => q.id === tab.savedQueryId)
				: null;

			// Pre-populate query name
			if (savedQuery) {
				queryName = savedQuery.name;
			} else if (tab) {
				queryName = tab.name;
			}

			// Extract parameters from query
			const paramNames = extractParameters(query);

			if (paramNames.length > 0) {
				// Use existing parameter definitions if available, otherwise create defaults
				parameterConfigs = paramNames.map((name) => {
					const existing = savedQuery?.parameters?.find((p) => p.name === name);
					return (
						existing ?? {
							name,
							type: "text" as QueryParameterType,
							defaultValue: undefined,
							description: undefined
						}
					);
				});
				paramsOpen = true;
			} else {
				parameterConfigs = [];
				paramsOpen = false;
			}
		}
	});

	const handleSave = () => {
		if (!queryName.trim()) {
			toast.error(m.save_query_error_name());
			return;
		}

		// Pass parameters if any exist
		const params = parameterConfigs.length > 0 ? parameterConfigs : undefined;
		db.savedQueries.saveQuery(queryName.trim(), query, tabId, params);
		toast.success(m.save_query_success());
		open = false;
		queryName = "";
		onSaveComplete?.();
	};

	const handleKeydown = (e: KeyboardEvent) => {
		if (e.key === "Enter" && !e.shiftKey) {
			handleSave();
		}
	};

	const paramTypeLabels: Record<QueryParameterType, string> = {
		text: "Text",
		number: "Number",
		date: "Date",
		datetime: "DateTime",
		boolean: "Boolean"
	};
</script>

<Dialog bind:open>
	<DialogContent class="max-w-lg max-h-[80vh] overflow-hidden flex flex-col">
		<DialogHeader>
			<DialogTitle>{m.save_query_title()}</DialogTitle>
			<DialogDescription>{m.save_query_description()}</DialogDescription>
		</DialogHeader>

		<div class="overflow-y-auto flex-1 py-4">
			<div class="grid gap-4">
				<div class="grid gap-2">
					<Label for="query-name">{m.save_query_label()}</Label>
					<Input
						id="query-name"
						bind:value={queryName}
						placeholder={m.save_query_placeholder()}
						onkeydown={handleKeydown}
					/>
				</div>

				{#if parameterConfigs.length > 0}
					<Collapsible bind:open={paramsOpen} class="space-y-2">
						<CollapsibleTrigger class="flex items-center gap-2 text-sm font-medium w-full">
							<ChevronDownIcon
								class="size-4 transition-transform duration-200 {paramsOpen
									? 'rotate-0'
									: '-rotate-90'}"
							/>
							{m.save_query_params_section()} ({parameterConfigs.length})
						</CollapsibleTrigger>
						<CollapsibleContent class="space-y-3 pt-2">
							<p class="text-xs text-muted-foreground">
								{m.save_query_params_description()}
							</p>
							{#each parameterConfigs as param, i (param.name)}
								<div class="rounded-lg border p-3 space-y-3">
									<div class="font-medium text-sm">{param.name}</div>
									<div class="grid grid-cols-2 gap-3">
										<div class="space-y-1">
											<Label class="text-xs text-muted-foreground"
												>{m.save_query_params_type()}</Label
											>
											<Select
												type="single"
												value={param.type}
												onValueChange={(v) => {
													if (v) {
														parameterConfigs[i] = { ...param, type: v as QueryParameterType };
													}
												}}
											>
												<SelectTrigger class="h-8">
													{paramTypeLabels[param.type]}
												</SelectTrigger>
												<SelectContent>
													{#each Object.entries(paramTypeLabels) as [value, label]}
														<SelectItem {value}>{label}</SelectItem>
													{/each}
												</SelectContent>
											</Select>
										</div>
										<div class="space-y-1">
											<Label class="text-xs text-muted-foreground"
												>{m.save_query_params_default()}</Label
											>
											<Input
												class="h-8"
												bind:value={parameterConfigs[i].defaultValue}
												placeholder="Optional"
											/>
										</div>
									</div>
								</div>
							{/each}
						</CollapsibleContent>
					</Collapsible>
				{/if}
			</div>
		</div>

		<DialogFooter>
			<Button variant="outline" onclick={() => (open = false)}>{m.save_query_cancel()}</Button>
			<Button onclick={handleSave}>{m.save_query_save()}</Button>
		</DialogFooter>
	</DialogContent>
</Dialog>
