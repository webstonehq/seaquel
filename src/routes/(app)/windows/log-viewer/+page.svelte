<script lang="ts">
	import { onMount, tick } from "svelte";
	import { m } from "$lib/paraglide/messages.js";
	import { Button } from "$lib/components/ui/button";
	import { Input } from "$lib/components/ui/input";
	import * as Select from "$lib/components/ui/select";
	import { readLogFile, clearLogFile } from "$lib/api/tauri";
	import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";
	import SearchIcon from "@lucide/svelte/icons/search";
	import XIcon from "@lucide/svelte/icons/x";
	import Trash2Icon from "@lucide/svelte/icons/trash-2";

	interface LogEntry {
		raw: string;
		ts: string;
		level: string;
		module: string;
		activity: string;
		msg: string;
		kv: Record<string, string>;
	}

	let logContent = $state("");
	let loading = $state(true);
	let error = $state("");
	// oxlint-disable-next-line eslint(no-unassigned-vars)
	let scrollContainer: HTMLDivElement;

	let levelFilter = $state("all");
	let searchQuery = $state("");

	function parseLogfmt(line: string): LogEntry {
		const entry: LogEntry = {
			raw: line,
			ts: "",
			level: "",
			module: "",
			activity: "",
			msg: "",
			kv: {},
		};

		let i = 0;
		while (i < line.length) {
			// skip whitespace
			while (i < line.length && line[i] === " ") i++;
			if (i >= line.length) break;

			// read key
			const keyStart = i;
			while (i < line.length && line[i] !== "=") i++;
			if (i >= line.length) break;
			const key = line.slice(keyStart, i);
			i++; // skip '='

			// read value
			let value: string;
			if (line[i] === '"') {
				// quoted value
				i++; // skip opening quote
				let valueChars = "";
				while (i < line.length) {
					if (line[i] === "\\" && i + 1 < line.length && line[i + 1] === '"') {
						valueChars += '"';
						i += 2;
					} else if (line[i] === '"') {
						i++; // skip closing quote
						break;
					} else {
						valueChars += line[i];
						i++;
					}
				}
				value = valueChars;
			} else {
				const valueStart = i;
				while (i < line.length && line[i] !== " ") i++;
				value = line.slice(valueStart, i);
			}

			if (key === "ts") entry.ts = value;
			else if (key === "level") entry.level = value;
			else if (key === "module") entry.module = value;
			else if (key === "activity") entry.activity = value;
			else if (key === "msg") entry.msg = value;
			entry.kv[key] = value;
		}

		return entry;
	}

	let entries = $derived.by(() => {
		if (!logContent) return [];
		return logContent
			.split("\n")
			.filter((line) => line.trim().length > 0)
			.map(parseLogfmt)
			.filter((e) => e.level !== "");
	});

	let filteredEntries = $derived.by(() => {
		let result = entries;

		if (levelFilter !== "all") {
			result = result.filter((e) => e.level === levelFilter);
		}

		if (searchQuery.trim()) {
			const q = searchQuery.toLowerCase();
			result = result.filter((e) => e.raw.toLowerCase().includes(q));
		}

		return result;
	});

	const levelCounts = $derived.by(() => {
		const counts: Record<string, number> = {
			ERROR: 0,
			WARN: 0,
			INFO: 0,
			DEBUG: 0,
			TRACE: 0,
		};
		for (const e of entries) {
			if (e.level in counts) counts[e.level]++;
		}
		return counts;
	});

	async function scrollToBottom() {
		await tick();
		if (scrollContainer) {
			scrollContainer.scrollTop = scrollContainer.scrollHeight;
		}
	}

	async function clearLogs() {
		try {
			await clearLogFile();
			logContent = "";
		} catch (e) {
			error = `${e}`;
		}
	}

	async function loadLogs() {
		loading = true;
		error = "";
		try {
			logContent = await readLogFile();
		} catch (e) {
			error = `${e}`;
		} finally {
			loading = false;
			await scrollToBottom();
		}
	}

	function levelColor(level: string): string {
		switch (level) {
			case "ERROR":
				return "text-red-500";
			case "WARN":
				return "text-yellow-500";
			case "INFO":
				return "text-blue-500";
			case "DEBUG":
				return "text-muted-foreground";
			case "TRACE":
				return "text-muted-foreground/60";
			default:
				return "";
		}
	}

	onMount(() => {
		loadLogs();
	});
</script>

<div class="flex flex-col h-screen bg-background text-foreground">
	<header
		class="flex items-center gap-2 border-b px-3 py-2 shrink-0"
		data-tauri-drag-region
	>
		<h1 class="text-sm font-medium shrink-0">{m.settings_view_logs()}</h1>

		<div class="flex-1"></div>

		<div class="relative">
			<SearchIcon class="absolute left-2 top-1/2 -translate-y-1/2 size-3.5 text-muted-foreground pointer-events-none" />
			<Input
				type="search"
				placeholder="Search logs..."
				class="h-7 w-52 pl-7 pr-7 text-xs"
				bind:value={searchQuery}
			/>
			{#if searchQuery}
				<button
					class="absolute right-1.5 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground"
					onclick={() => (searchQuery = "")}
				>
					<XIcon class="size-3.5" />
				</button>
			{/if}
		</div>

		<Select.Root type="single" value={levelFilter} onValueChange={(v) => (levelFilter = v)}>
			<Select.Trigger size="sm" class="!h-7 text-xs w-28 py-0">
				{levelFilter === "all" ? "All levels" : levelFilter}
			</Select.Trigger>
			<Select.Content>
				<Select.Item value="all">All levels</Select.Item>
				<Select.Item value="ERROR">ERROR ({levelCounts.ERROR})</Select.Item>
				<Select.Item value="WARN">WARN ({levelCounts.WARN})</Select.Item>
				<Select.Item value="INFO">INFO ({levelCounts.INFO})</Select.Item>
				<Select.Item value="DEBUG">DEBUG ({levelCounts.DEBUG})</Select.Item>
				<Select.Item value="TRACE">TRACE ({levelCounts.TRACE})</Select.Item>
			</Select.Content>
		</Select.Root>

		<Button variant="ghost" size="sm" class="h-7 px-2" onclick={clearLogs} disabled={loading}>
			<Trash2Icon class="size-3.5" />
		</Button>
		<Button variant="ghost" size="sm" class="h-7 px-2" onclick={loadLogs} disabled={loading}>
			<RefreshCwIcon class="size-3.5" />
		</Button>
	</header>

	<div class="flex-1 overflow-auto min-h-0" bind:this={scrollContainer}>
		{#if loading}
			<p class="text-sm text-muted-foreground p-4">Loading...</p>
		{:else if error}
			<p class="text-sm text-destructive p-4">{error}</p>
		{:else if filteredEntries.length === 0}
			<p class="text-sm text-muted-foreground p-4">No matching log entries.</p>
		{:else}
			<div class="text-xs font-mono grid [&_*]:select-text" style="grid-template-columns: auto auto auto 1fr;">
				{#each filteredEntries as entry, i (i)}
					<span class="px-2 py-0.5 text-muted-foreground whitespace-nowrap border-b border-border/40">{entry.ts.slice(11) || ""}</span>
					<span class="px-1.5 py-0.5 whitespace-nowrap font-semibold border-b border-border/40 {levelColor(entry.level)}">{entry.level}</span>
					<span class="px-1.5 py-0.5 text-muted-foreground whitespace-nowrap border-b border-border/40">{entry.activity || ""}</span>
					<span class="px-2 py-0.5 break-all border-b border-border/40">{entry.msg}{#each Object.entries(entry.kv).filter(([k]) => !["ts", "level", "module", "activity", "msg"].includes(k)) as [key, value] (key)}{" "}<span class="text-muted-foreground">{key}=<span class="text-foreground/70">{value}</span></span>{/each}</span>
				{/each}
			</div>
		{/if}
	</div>

	{#if !loading && !error}
		<footer class="border-t px-3 py-1 text-xs text-muted-foreground shrink-0">
			{filteredEntries.length} of {entries.length} entries
		</footer>
	{/if}
</div>
