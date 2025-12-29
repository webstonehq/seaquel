<script lang="ts">
	import { Button } from "$lib/components/ui/button/index.js";
	import * as Card from "$lib/components/ui/card/index.js";
	import DatabaseIcon from "@lucide/svelte/icons/database";
	import FileCodeIcon from "@lucide/svelte/icons/file-code";
	import TableIcon from "@lucide/svelte/icons/table";
	import NetworkIcon from "@lucide/svelte/icons/network";
	import BotIcon from "@lucide/svelte/icons/bot";
	import PlusIcon from "@lucide/svelte/icons/plus";
	import { connectionDialogStore } from "$lib/stores/connection-dialog.svelte.js";

	const features = [
		{
			icon: FileCodeIcon,
			title: "Query Editor",
			description: "Write and execute SQL queries with syntax highlighting",
		},
		{
			icon: TableIcon,
			title: "Schema Browser",
			description: "Explore tables, columns, and relationships",
		},
		{
			icon: NetworkIcon,
			title: "ERD Viewer",
			description: "Visualize your database structure",
		},
		{
			icon: BotIcon,
			title: "AI Assistant",
			description: "Get help writing queries with AI",
		},
	];
</script>

<div class="flex-1 flex items-center justify-center p-8">
	<div class="max-w-2xl w-full space-y-8">
		<div class="text-center space-y-4">
			<div class="flex justify-center">
				<div class="p-4 rounded-full bg-muted">
					<DatabaseIcon class="size-12 text-muted-foreground" />
				</div>
			</div>
			<div>
				<h1 class="text-2xl font-semibold">Welcome to Seaquel</h1>
				<p class="text-muted-foreground mt-1">Connect to a database to get started</p>
			</div>
			<Button size="lg" onclick={() => connectionDialogStore.open()}>
				<PlusIcon class="size-4 mr-2" />
				Add Your First Connection
			</Button>
		</div>

		<div class="grid grid-cols-2 gap-4">
			{#each features as feature}
				<Card.Root class="bg-muted/30">
					<Card.Header class="pb-2">
						<div class="flex items-center gap-2">
							<feature.icon class="size-4 text-muted-foreground" />
							<Card.Title class="text-sm font-medium">{feature.title}</Card.Title>
						</div>
					</Card.Header>
					<Card.Content class="pt-0">
						<p class="text-xs text-muted-foreground">{feature.description}</p>
					</Card.Content>
				</Card.Root>
			{/each}
		</div>
	</div>
</div>
