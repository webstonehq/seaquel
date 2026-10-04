<script lang="ts">
	import { onMount } from "svelte";
	import { SvelteSet } from "svelte/reactivity";
	import { platform } from "@tauri-apps/plugin-os";
	import { m } from "$lib/paraglide/messages.js";
	import { Button } from "$lib/components/ui/button";
	import { Checkbox } from "$lib/components/ui/checkbox";
	import { errorToast } from "$lib/utils/toast";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { getCliInfo, installCli, type CliInfo } from "$lib/api/tauri";
	import McpSnippet from "./mcp-snippet.svelte";
	import { helperNeeded, offersInstall } from "./cli-status";
	import {
		claudeCodeCommand,
		claudeDesktopConfig,
		commandLine,
		type McpSelection,
		type ShellKind,
	} from "./mcp-snippets";
	import CheckIcon from "@lucide/svelte/icons/check";
	import TriangleAlertIcon from "@lucide/svelte/icons/triangle-alert";
	import InfoIcon from "@lucide/svelte/icons/info";
	import ShieldCheckIcon from "@lucide/svelte/icons/shield-check";

	const db = useDatabase();

	function currentPlatform(): string {
		try {
			return platform();
		} catch {
			return "";
		}
	}
	const os = currentPlatform();
	const shell: ShellKind = os === "windows" ? "powershell" : "posix";

	let info = $state<CliInfo | null>(null);
	let loadError = $state<string | null>(null);
	let installing = $state(false);

	const selectedProjects = new SvelteSet<string>();
	const selectedConnections = new SvelteSet<string>();

	async function refresh() {
		try {
			info = await getCliInfo();
			loadError = null;
		} catch (error) {
			loadError = error instanceof Error ? error.message : String(error);
		}
	}

	onMount(() => {
		void refresh();
	});

	async function install() {
		installing = true;
		try {
			await installCli();
		} catch (error) {
			errorToast(error instanceof Error ? error.message : String(error));
		} finally {
			installing = false;
			await refresh();
		}
	}

	const groups = $derived(
		db.state.projects.map((project) => ({
			project,
			connections: db.state.connections.filter((c) => c.projectId === project.id),
		})),
	);

	// In display order, so the command doesn't depend on click order. A
	// connection in a project exposed whole isn't listed again.
	const selection: McpSelection = $derived({
		projectIds: groups.filter((g) => selectedProjects.has(g.project.id)).map((g) => g.project.id),
		connectionIds: groups
			.filter((g) => !selectedProjects.has(g.project.id))
			.flatMap((g) => g.connections)
			.filter((c) => selectedConnections.has(c.id))
			.map((c) => c.id),
	});

	// The CLI's DuckDB helper (installed beside it by the same button).
	const needsHelper = $derived(helperNeeded(info));

	const nothingSelected = $derived(selection.projectIds.length === 0 && selection.connectionIds.length === 0);
	const binary = $derived(info?.commandPath ?? "seaquel-cli");

	function toggle(set: SvelteSet<string>, id: string, checked: boolean) {
		if (checked) set.add(id);
		else set.delete(id);
	}
</script>

<svelte:window onfocus={() => void refresh()} />

<div class="space-y-6" data-section="mcp">
	<div>
		<h2 class="text-lg font-medium">{m.settings_mcp()}</h2>
		<p class="text-sm text-muted-foreground mt-1">{m.settings_mcp_description()}</p>
	</div>

	<div class="flex gap-2 text-xs text-muted-foreground bg-muted rounded-md px-3 py-2">
		<ShieldCheckIcon class="size-4 shrink-0" />
		<p>{m.settings_mcp_read_only_note()}</p>
	</div>

	<div class="space-y-2">
		<p class="text-sm font-medium">{m.settings_mcp_binary()}</p>
		{#if loadError}
			<p class="text-sm text-destructive">{m.settings_mcp_load_failed({ error: loadError })}</p>
		{:else if info}
			<pre class="bg-muted rounded-md px-3 py-2 text-xs font-mono break-all whitespace-pre-wrap select-text">{info.binaryPath}</pre>
			{#if !info.binaryExists}
				<p class="flex items-center gap-1.5 text-xs text-destructive">
					<TriangleAlertIcon class="size-3.5 shrink-0" />
					{m.settings_mcp_binary_missing()}
				</p>
			{:else if info.pathStatus === "installed"}
				<p class="flex items-center gap-1.5 text-xs text-muted-foreground">
					<CheckIcon class="size-3.5 shrink-0 text-green-600" />
					{m.settings_mcp_path_installed()}
				</p>
			{:else if info.pathStatus === "outdated"}
				<p class="flex items-center gap-1.5 text-xs text-amber-600">
					<TriangleAlertIcon class="size-3.5 shrink-0" />
					{m.settings_mcp_path_outdated()}
				</p>
			{:else if info.pathStatus === "other"}
				<p class="flex items-center gap-1.5 text-xs text-amber-600 break-all">
					<TriangleAlertIcon class="size-3.5 shrink-0" />
					{m.settings_mcp_path_other({ path: info.foundPath ?? "" })}
				</p>
			{:else}
				<p class="flex items-center gap-1.5 text-xs text-muted-foreground">
					<InfoIcon class="size-3.5 shrink-0" />
					{info.appImage ? m.settings_mcp_path_missing_appimage() : m.settings_mcp_path_missing()}
				</p>
			{/if}
			{#if needsHelper}
				<p class="flex items-center gap-1.5 text-xs text-amber-600">
					<TriangleAlertIcon class="size-3.5 shrink-0" />
					{info.duckdbHelper === "unsafe" ? m.settings_mcp_duckdb_unsafe() : m.settings_mcp_duckdb_missing()}
				</p>
			{/if}
			{#if offersInstall(info, os)}
				<Button variant="outline" size="sm" onclick={install} disabled={installing}>
					{installing ? m.settings_mcp_installing() : m.settings_mcp_install()}
				</Button>
			{/if}
		{/if}
	</div>

	{#if os === "macos"}
		<div class="flex gap-2 text-xs text-muted-foreground bg-muted rounded-md px-3 py-2">
			<InfoIcon class="size-4 shrink-0" />
			<p>{m.settings_mcp_keychain_note()}</p>
		</div>
	{/if}

	<div class="space-y-3">
		<div>
			<p class="text-sm font-medium">{m.settings_mcp_expose()}</p>
			<p class="text-xs text-muted-foreground">{m.settings_mcp_expose_description()}</p>
		</div>
		<div class="max-h-80 overflow-y-auto space-y-3 border rounded-lg p-3">
			{#each groups as group (group.project.id)}
				{@const whole = selectedProjects.has(group.project.id)}
				<div class="space-y-1.5">
					<div class="flex items-center justify-between gap-2">
						<span class="text-sm font-medium truncate">{group.project.name}</span>
						<label class="flex items-center gap-2 text-xs text-muted-foreground cursor-pointer shrink-0">
							<Checkbox
								checked={whole}
								onCheckedChange={(checked) => toggle(selectedProjects, group.project.id, checked === true)}
							/>
							{m.settings_mcp_whole_project()}
						</label>
					</div>
					{#if group.connections.length === 0}
						<p class="text-xs text-muted-foreground pl-1">{m.settings_mcp_project_empty()}</p>
					{:else}
						<div class="space-y-1 pl-1">
							{#each group.connections as connection (connection.id)}
								<label
									class="flex items-center gap-2 text-sm cursor-pointer"
									class:opacity-60={whole}
								>
									<Checkbox
										checked={whole || selectedConnections.has(connection.id)}
										disabled={whole}
										onCheckedChange={(checked) => toggle(selectedConnections, connection.id, checked === true)}
									/>
									<span class="truncate">{connection.name}</span>
									<span class="text-xs text-muted-foreground uppercase shrink-0">{connection.type}</span>
								</label>
							{/each}
						</div>
					{/if}
				</div>
			{/each}
		</div>
		{#if nothingSelected}
			<p class="text-xs text-muted-foreground">{m.settings_mcp_nothing_selected()}</p>
		{/if}
	</div>

	<McpSnippet
		title={m.settings_mcp_claude_desktop()}
		hint={m.settings_mcp_claude_desktop_hint()}
		text={claudeDesktopConfig(binary, selection)}
	/>
	<McpSnippet
		title={m.settings_mcp_claude_code()}
		hint={m.settings_mcp_claude_code_hint()}
		text={claudeCodeCommand(binary, selection, shell)}
	/>
	<McpSnippet title={m.settings_mcp_command()} text={commandLine(binary, selection, shell)} />
</div>
