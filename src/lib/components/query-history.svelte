<script lang="ts">
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "$lib/components/ui/card";
	import { Button } from "$lib/components/ui/button";
	import { Badge } from "$lib/components/ui/badge";
	import { Input } from "$lib/components/ui/input";
	import { ScrollArea } from "$lib/components/ui/scroll-area";
	import { Tabs, TabsContent, TabsList, TabsTrigger } from "$lib/components/ui/tabs";
	import { HistoryIcon, StarIcon, SearchIcon, ClockIcon, BookmarkIcon, Trash2Icon } from "@lucide/svelte";

	const db = useDatabase();
	let searchQuery = $state("");

	const filteredHistory = $derived(db.activeConnectionQueryHistory.filter((item) => item.query.toLowerCase().includes(searchQuery.toLowerCase())));

	const filteredSavedQueries = $derived(db.activeConnectionSavedQueries.filter((item) => item.name.toLowerCase().includes(searchQuery.toLowerCase()) || item.query.toLowerCase().includes(searchQuery.toLowerCase())));

	const formatTime = (date: Date) => {
		const now = new Date();
		const diff = now.getTime() - date.getTime();
		const minutes = Math.floor(diff / 60000);
		const hours = Math.floor(diff / 3600000);
		const days = Math.floor(diff / 86400000);

		if (minutes < 1) return "Just now";
		if (minutes < 60) return `${minutes}m ago`;
		if (hours < 24) return `${hours}h ago`;
		return `${days}d ago`;
	};
</script>

<Card class="h-full flex flex-col">
	<CardHeader class="pb-3">
		<div class="flex items-center justify-between">
			<div>
				<CardTitle class="text-base flex items-center gap-2">
					<HistoryIcon class="size-4" />
					Queries
				</CardTitle>
				<CardDescription class="text-xs">{db.activeConnectionQueryHistory.length} executed • {db.activeConnectionSavedQueries.length} saved</CardDescription>
			</div>
		</div>
		<div class="relative mt-2">
			<SearchIcon class="absolute left-2 top-1/2 -translate-y-1/2 size-4 text-muted-foreground" />
			<Input bind:value={searchQuery} placeholder="Search queries..." class="pl-8 h-8 text-sm" />
		</div>
	</CardHeader>

	<CardContent class="flex-1 overflow-hidden p-0">
		<Tabs value="history" class="h-full flex flex-col">
			<TabsList class="w-full justify-start rounded-none border-b h-9 bg-transparent px-4">
				<TabsTrigger value="history" class="text-xs data-[state=active]:bg-background">History</TabsTrigger>
				<TabsTrigger value="saved" class="text-xs data-[state=active]:bg-background">Saved</TabsTrigger>
			</TabsList>

			<TabsContent value="history" class="flex-1 overflow-hidden m-0">
				<ScrollArea class="h-full">
					<div class="p-4 flex flex-col gap-2">
						{#each filteredHistory as item (item.id)}
							<div class="w-full text-left p-3 rounded-lg border hover:bg-muted/50 transition-colors group" onclick={() => db.loadQueryFromHistory(item.id)}>
								<div class="flex items-start justify-between gap-2 mb-2">
									<div class="flex items-center gap-2 flex-1 min-w-0">
										<ClockIcon class="size-3 text-muted-foreground shrink-0" />
										<span class="text-xs text-muted-foreground">{formatTime(item.timestamp)}</span>
										<Badge variant="secondary" class="text-xs">{item.executionTime}ms</Badge>
										<Badge variant="outline" class="text-xs">{item.rowCount} rows</Badge>
									</div>
									<Button
										size="icon"
										variant="ghost"
										class={["size-6 shrink-0 [&_svg:not([class*='size-'])]:size-3", item.favorite && "text-yellow-500"]}
										onclick={(e) => {
											e.stopPropagation();
											db.toggleQueryFavorite(item.id);
										}}
									>
										<StarIcon class={[item.favorite && "fill-current"]} />
									</Button>
								</div>
								<p class="text-sm font-mono line-clamp-2 text-muted-foreground group-hover:text-foreground transition-colors">
									{item.query}
								</p>
							</div>
						{/each}

						{#if filteredHistory.length === 0}
							<div class="text-center py-8 text-muted-foreground">
								<HistoryIcon class="size-12 mx-auto mb-2 opacity-20" />
								<p class="text-sm">No query history found</p>
							</div>
						{/if}
					</div>
				</ScrollArea>
			</TabsContent>

			<TabsContent value="saved" class="flex-1 overflow-hidden m-0">
				<ScrollArea class="h-full">
					<div class="p-4 flex flex-col gap-2">
						{#each filteredSavedQueries as item (item.id)}
							<div class="w-full text-left p-3 rounded-lg border hover:bg-muted/50 transition-colors group" onclick={() => db.loadSavedQuery(item.id)}>
								<div class="flex items-start justify-between gap-2 mb-2">
									<div class="flex items-center gap-2 flex-1 min-w-0">
										<BookmarkIcon class="size-3 text-primary shrink-0" />
										<span class="text-sm font-medium truncate">{item.name}</span>
									</div>
									<Button
										size="icon"
										variant="ghost"
										class="size-6 shrink-0 [&_svg:not([class*='size-'])]:size-3 opacity-0 group-hover:opacity-100 transition-opacity hover:text-destructive"
										onclick={(e) => {
											e.stopPropagation();
											db.deleteSavedQuery(item.id);
										}}
									>
										<Trash2Icon />
									</Button>
								</div>
								<p class="text-xs text-muted-foreground mb-2">Updated {formatTime(item.updatedAt)}</p>
								<p class="text-xs font-mono line-clamp-2 text-muted-foreground group-hover:text-foreground transition-colors">
									{item.query}
								</p>
							</div>
						{/each}

						{#if filteredSavedQueries.length === 0}
							<div class="text-center py-8 text-muted-foreground">
								<BookmarkIcon class="size-12 mx-auto mb-2 opacity-20" />
								<p class="text-sm">No saved queries found</p>
								<p class="text-xs mt-1">Save queries to access them later</p>
							</div>
						{/if}
					</div>
				</ScrollArea>
			</TabsContent>
		</Tabs>
	</CardContent>
</Card>
