<script lang="ts">
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
	import type { QueryParameter, ParameterValue } from "$lib/types";
	import { coerceValue } from "$lib/db/query-params";
	import { m } from "$lib/paraglide/messages.js";

	type Props = {
		open?: boolean;
		parameters: QueryParameter[];
		onExecute: (values: ParameterValue[]) => void;
		onCancel: () => void;
	};

	let { open = $bindable(false), parameters, onExecute, onCancel }: Props = $props();

	// Store string values for inputs
	let values = $state<Record<string, string>>({});

	// Initialize values with defaults when dialog opens
	$effect(() => {
		if (open) {
			const initial: Record<string, string> = {};
			parameters.forEach((p) => {
				initial[p.name] = p.defaultValue ?? "";
			});
			values = initial;
		}
	});

	const handleExecute = () => {
		const paramValues: ParameterValue[] = parameters.map((p) => ({
			name: p.name,
			value: coerceValue(values[p.name] ?? "", p.type)
		}));
		onExecute(paramValues);
		open = false;
	};

	const handleCancel = () => {
		open = false;
		onCancel();
	};

	const handleKeydown = (e: KeyboardEvent) => {
		if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
			e.preventDefault();
			handleExecute();
		}
	};

	const getInputType = (type: QueryParameter["type"]): string => {
		switch (type) {
			case "number":
				return "number";
			case "date":
				return "date";
			case "datetime":
				return "datetime-local";
			default:
				return "text";
		}
	};

	const getPlaceholder = (param: QueryParameter): string => {
		return param.description ?? m.params_placeholder({ name: param.name });
	};
</script>

<Dialog bind:open>
	<DialogContent class="max-w-md max-h-[80vh] overflow-hidden flex flex-col">
		<DialogHeader>
			<DialogTitle>{m.params_dialog_title()}</DialogTitle>
			<DialogDescription>{m.params_dialog_description()}</DialogDescription>
		</DialogHeader>

		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<div class="overflow-y-auto flex-1 py-4" onkeydown={handleKeydown}>
			<div class="grid gap-4">
				{#each parameters as param (param.name)}
					<div class="grid gap-2">
						<Label for={param.name} class="flex items-center gap-2">
							{param.name}
							<span class="text-xs text-muted-foreground font-normal">({param.type})</span>
						</Label>
						{#if param.type === "boolean"}
							<Select
								type="single"
								value={values[param.name]}
								onValueChange={(v) => {
									if (v) values[param.name] = v;
								}}
							>
								<SelectTrigger class="w-full">
									{values[param.name] === "true"
										? "True"
										: values[param.name] === "false"
											? "False"
											: m.params_select_value()}
								</SelectTrigger>
								<SelectContent>
									<SelectItem value="true">True</SelectItem>
									<SelectItem value="false">False</SelectItem>
								</SelectContent>
							</Select>
						{:else}
							<Input
								id={param.name}
								type={getInputType(param.type)}
								bind:value={values[param.name]}
								placeholder={getPlaceholder(param)}
							/>
						{/if}
					</div>
				{/each}
			</div>
		</div>

		<DialogFooter>
			<Button variant="outline" onclick={handleCancel}>{m.params_cancel()}</Button>
			<Button onclick={handleExecute}>{m.params_execute()}</Button>
		</DialogFooter>
	</DialogContent>
</Dialog>
