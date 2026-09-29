<script lang="ts">
	import { Tabs, TabsList, TabsTrigger } from "$lib/components/ui/tabs";
	import DatabaseIcon from "@lucide/svelte/icons/database";
	import FileTextIcon from "@lucide/svelte/icons/file-text";
	import LayoutDashboardIcon from "@lucide/svelte/icons/layout-dashboard";
	import * as Tooltip from "$lib/components/ui/tooltip/index.js";
	import { m } from "$lib/paraglide/messages.js";

	interface Props {
		value: "schema" | "queries" | "dashboards";
		hasActiveConnection: boolean;
	}

	let { value = $bindable(), hasActiveConnection }: Props = $props();
</script>

<Tabs bind:value class="w-full px-2">
	<TabsList class="w-full justify-start rounded-none h-10 bg-transparent px-2">
		{#if hasActiveConnection}
		<Tooltip.Root>
			<Tooltip.Trigger>
				{#snippet child({ props })}
					<TabsTrigger {...props} value="schema" class="text-xs data-[state=active]:bg-background">
						<DatabaseIcon class="size-3" />
						{#if value === "schema"}
							{m.sidebar_tab_schema()}
						{/if}
					</TabsTrigger>
				{/snippet}
			</Tooltip.Trigger>
			{#if value !== "schema"}
				<Tooltip.Content>{m.sidebar_tab_schema()}</Tooltip.Content>
			{/if}
		</Tooltip.Root>
		{/if}
		<Tooltip.Root>
			<Tooltip.Trigger>
				{#snippet child({ props })}
					<TabsTrigger {...props} value="queries" class="text-xs data-[state=active]:bg-background">
						<FileTextIcon class="size-3" />
						{#if value === "queries"}
							{m.sidebar_tab_queries()}
						{/if}
					</TabsTrigger>
				{/snippet}
			</Tooltip.Trigger>
			{#if value !== "queries"}
				<Tooltip.Content>{m.sidebar_tab_queries()}</Tooltip.Content>
			{/if}
		</Tooltip.Root>
		<Tooltip.Root>
			<Tooltip.Trigger>
				{#snippet child({ props })}
					<TabsTrigger {...props} value="dashboards" class="text-xs data-[state=active]:bg-background">
						<LayoutDashboardIcon class="size-3" />
						{#if value === "dashboards"}
							Dashboards
						{/if}
					</TabsTrigger>
				{/snippet}
			</Tooltip.Trigger>
			{#if value !== "dashboards"}
				<Tooltip.Content>Dashboards</Tooltip.Content>
			{/if}
		</Tooltip.Root>
	</TabsList>
</Tabs>
