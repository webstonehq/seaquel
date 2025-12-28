<script lang="ts">
  import { useDatabase } from "$lib/hooks/database.svelte.js";
  import { SvelteFlow, Background, Controls, MiniMap } from "@xyflow/svelte";
  import "@xyflow/svelte/dist/style.css";
  import { toPng } from "html-to-image";
  import { generateErdSvg } from "$lib/utils/erd-svg-export";
  import { toast } from "svelte-sonner";
  import { save } from "@tauri-apps/plugin-dialog";
  import { writeFile, writeTextFile, remove } from "@tauri-apps/plugin-fs";
  import { tempDir } from "@tauri-apps/api/path";
  import { writeText } from "@tauri-apps/plugin-clipboard-manager";
  import { invoke } from "@tauri-apps/api/core";
  import { Input } from "$lib/components/ui/input";
  import { Button } from "$lib/components/ui/button";
  import { Checkbox } from "$lib/components/ui/checkbox";
  import * as Popover from "$lib/components/ui/popover";
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu";
  import {
    DatabaseIcon,
    TableIcon,
    LinkIcon,
    SearchIcon,
    FilterIcon,
    DownloadIcon,
    ImageIcon,
    FileCodeIcon,
    ClipboardIcon,
  } from "@lucide/svelte";
  import ErdTableNode from "./erd-table-node.svelte";
  import { layoutErdDiagram } from "$lib/utils/erd-layout";
  import type { Node, Edge, NodeTypes, ColorMode } from "@xyflow/svelte";
  import { mode } from "mode-watcher";

  const db = useDatabase();

  // Map mode-watcher theme to xyflow colorMode
  const colorMode: ColorMode = $derived(mode.current === "dark" ? "dark" : "light");

  // Custom node types
  const nodeTypes: NodeTypes = {
    erdTableNode: ErdTableNode,
  };

  // Search state
  let searchQuery = $state("");

  // Schema filter state
  const allSchemas = $derived([...new Set(db.activeSchema?.map(t => t.schema) || [])].sort());
  let visibleSchemas = $state<Set<string>>(new Set());

  // Initialize visible schemas when allSchemas changes
  $effect(() => {
    if (allSchemas.length > 0 && visibleSchemas.size === 0) {
      visibleSchemas = new Set(allSchemas);
    }
  });

  // Filtered tables based on search and schema filter
  const filteredTables = $derived.by(() => {
    if (!db.activeSchema) return [];

    let tables = db.activeSchema;

    // Apply schema filter (only if not all schemas are selected)
    if (visibleSchemas.size > 0 && visibleSchemas.size < allSchemas.length) {
      tables = tables.filter(t => visibleSchemas.has(t.schema));
    }

    // Apply search filter
    if (searchQuery.trim()) {
      const query = searchQuery.toLowerCase();
      tables = tables.filter(t =>
        t.name.toLowerCase().includes(query) ||
        t.schema.toLowerCase().includes(query)
      );
    }

    return tables;
  });

  // Convert filtered tables to xyflow nodes and edges
  const flowData = $derived.by(() => {
    if (filteredTables.length === 0) {
      return { nodes: [] as Node[], edges: [] as Edge[] };
    }
    return layoutErdDiagram(filteredTables);
  });

  let nodes = $derived(flowData.nodes);
  let edges = $derived(flowData.edges);

  // Calculate stats
  const totalTableCount = $derived(db.activeSchema?.length || 0);
  const filteredTableCount = $derived(filteredTables.length);
  const relationshipCount = $derived(edges.length);
  const isFiltered = $derived(filteredTableCount < totalTableCount);

  // Schema toggle helper
  const toggleSchema = (schema: string, checked: boolean | "indeterminate") => {
    const newSet = new Set(visibleSchemas);
    if (checked === true) {
      newSet.add(schema);
    } else {
      newSet.delete(schema);
    }
    visibleSchemas = newSet;
  };

  const selectAllSchemas = () => {
    visibleSchemas = new Set(allSchemas);
  };

  const clearAllSchemas = () => {
    visibleSchemas = new Set();
  };

  // Export functionality
  let flowContainer: HTMLElement;

  const getFlowElement = () => flowContainer?.querySelector('.svelte-flow') as HTMLElement | null;

  const getImageOptions = () => ({
    backgroundColor: mode.current === 'dark' ? '#0a0a0a' : '#ffffff',
    filter: (node: Element) => {
      // Exclude minimap and controls from export
      return !node.classList?.contains('svelte-flow__minimap') &&
             !node.classList?.contains('svelte-flow__controls');
    }
  });

  const exportToPng = async () => {
    const element = getFlowElement();
    if (!element) return;

    try {
      const filePath = await save({
        defaultPath: `erd-${db.activeConnection?.name || 'diagram'}.png`,
        filters: [{ name: 'PNG Image', extensions: ['png'] }]
      });
      if (!filePath) return;

      const dataUrl = await toPng(element, getImageOptions());
      const base64 = dataUrl.split(',')[1];
      const bytes = Uint8Array.from(atob(base64), c => c.charCodeAt(0));
      await writeFile(filePath, bytes);
      toast.success('PNG saved');
    } catch (e) {
      toast.error('Failed to export PNG');
    }
  };

  const exportToSvg = async () => {
    try {
      const filePath = await save({
        defaultPath: `erd-${db.activeConnection?.name || 'diagram'}.svg`,
        filters: [{ name: 'SVG Image', extensions: ['svg'] }]
      });
      if (!filePath) return;

      const svg = generateErdSvg(nodes, edges, {
        theme: mode.current === 'dark' ? 'dark' : 'light'
      });
      await writeTextFile(filePath, svg);
      toast.success('SVG saved');
    } catch (e) {
      toast.error('Failed to export SVG');
    }
  };

  const copyPngToClipboard = async () => {
    const element = getFlowElement();
    if (!element) return;

    try {
      const dataUrl = await toPng(element, getImageOptions());
      const base64 = dataUrl.split(',')[1];
      const bytes = Uint8Array.from(atob(base64), c => c.charCodeAt(0));

      // Write to temp file, copy from there
      const tempPath = await tempDir();
      const filePath = `${tempPath}erd-clipboard-${Date.now()}.png`;
      await writeFile(filePath, bytes);

      await invoke('copy_image_to_clipboard', { path: filePath });

      // Clean up temp file
      await remove(filePath).catch(() => {});

      toast.success('PNG copied to clipboard');
    } catch (e) {
      console.error('Failed to copy PNG:', e);
      toast.error('Failed to copy PNG');
    }
  };

  const copySvgToClipboard = async () => {
    try {
      const svg = generateErdSvg(nodes, edges, {
        theme: mode.current === 'dark' ? 'dark' : 'light'
      });
      await writeText(svg);
      toast.success('SVG copied to clipboard');
    } catch (e) {
      toast.error('Failed to copy SVG');
    }
  };
</script>

<div class="flex flex-col h-full">
  {#if db.activeErdTab}
    {#if db.activeSchema && db.activeSchema.length > 0}
      <!-- Summary Header -->
      <div class="p-4 border-b bg-muted/30 shrink-0">
        <div class="flex items-start justify-between mb-3">
          <div>
            <h2 class="text-lg font-semibold flex items-center gap-2">
              <DatabaseIcon class="size-5" />
              Entity Relationship Diagram
            </h2>
            <div class="flex items-center gap-4 mt-1 text-sm text-muted-foreground">
              <span class="flex items-center gap-1">
                <TableIcon class="size-3" />
                {#if isFiltered}
                  {filteredTableCount} / {totalTableCount} tables
                {:else}
                  {totalTableCount} tables
                {/if}
              </span>
              <span class="flex items-center gap-1">
                <LinkIcon class="size-3" />
                {relationshipCount} relationships
              </span>
            </div>
          </div>
        </div>

        <!-- Controls Row -->
        <div class="flex items-center gap-2 flex-wrap">
          <!-- Search Input -->
          <div class="relative">
            <SearchIcon class="absolute left-2.5 top-1/2 -translate-y-1/2 size-4 text-muted-foreground" />
            <Input
              type="text"
              placeholder="Search tables..."
              class="h-8 w-48 pl-8"
              bind:value={searchQuery}
            />
          </div>

          <!-- Schema Filter -->
          {#if allSchemas.length > 1}
            <Popover.Root>
              <Popover.Trigger>
                <Button variant="outline" size="sm" class="h-8">
                  <FilterIcon class="size-4 mr-2" />
                  Schemas ({visibleSchemas.size}/{allSchemas.length})
                </Button>
              </Popover.Trigger>
              <Popover.Content class="w-56">
                <div class="space-y-2">
                  <div class="flex items-center justify-between pb-2 border-b">
                    <span class="text-sm font-medium">Filter by Schema</span>
                    <div class="flex gap-1">
                      <Button variant="ghost" size="sm" class="h-6 px-2 text-xs" onclick={selectAllSchemas}>
                        All
                      </Button>
                      <Button variant="ghost" size="sm" class="h-6 px-2 text-xs" onclick={clearAllSchemas}>
                        None
                      </Button>
                    </div>
                  </div>
                  {#each allSchemas as schema}
                    <label class="flex items-center gap-2 cursor-pointer">
                      <Checkbox
                        checked={visibleSchemas.has(schema)}
                        onCheckedChange={(checked: boolean | "indeterminate") => toggleSchema(schema, checked)}
                      />
                      <span class="text-sm">{schema}</span>
                    </label>
                  {/each}
                </div>
              </Popover.Content>
            </Popover.Root>
          {/if}

          <!-- Export Dropdown -->
          <DropdownMenu.Root>
            <DropdownMenu.Trigger>
              <Button variant="outline" size="sm" class="h-8">
                <DownloadIcon class="size-4 mr-2" />
                Export
              </Button>
            </DropdownMenu.Trigger>
            <DropdownMenu.Content>
              <DropdownMenu.Label>Download</DropdownMenu.Label>
              <DropdownMenu.Item onclick={exportToPng}>
                <ImageIcon class="size-4 mr-2" />
                Download PNG
              </DropdownMenu.Item>
              <DropdownMenu.Item onclick={exportToSvg}>
                <FileCodeIcon class="size-4 mr-2" />
                Download SVG
              </DropdownMenu.Item>
              <DropdownMenu.Separator />
              <DropdownMenu.Label>Copy to Clipboard</DropdownMenu.Label>
              <DropdownMenu.Item onclick={copyPngToClipboard}>
                <ClipboardIcon class="size-4 mr-2" />
                Copy as PNG
              </DropdownMenu.Item>
              <DropdownMenu.Item onclick={copySvgToClipboard}>
                <ClipboardIcon class="size-4 mr-2" />
                Copy as SVG
              </DropdownMenu.Item>
            </DropdownMenu.Content>
          </DropdownMenu.Root>
        </div>
      </div>

      <!-- Flow Diagram -->
      <div class="flex-1 min-h-0" bind:this={flowContainer}>
        {#if filteredTables.length > 0}
          <SvelteFlow
            {nodes}
            {edges}
            {nodeTypes}
            {colorMode}
            fitView
            minZoom={0.1}
            maxZoom={2}
            nodesDraggable={true}
            nodesConnectable={false}
            elementsSelectable={true}
            deleteKey={null}
            proOptions={{ hideAttribution: true }}
          >
            <Background />
            <Controls />
            <MiniMap />
          </SvelteFlow>
        {:else}
          <div class="h-full flex items-center justify-center text-muted-foreground">
            <div class="text-center">
              <SearchIcon class="size-12 mx-auto mb-2 opacity-20" />
              <p class="text-sm">No tables match your filter</p>
              <Button
                variant="link"
                size="sm"
                class="mt-2"
                onclick={() => { searchQuery = ""; visibleSchemas = new Set(allSchemas); }}
              >
                Clear filters
              </Button>
            </div>
          </div>
        {/if}
      </div>
    {:else}
      <!-- No schema state -->
      <div class="flex-1 flex items-center justify-center text-muted-foreground">
        <div class="text-center">
          <DatabaseIcon class="size-12 mx-auto mb-2 opacity-20" />
          <p class="text-sm">No tables found in database</p>
        </div>
      </div>
    {/if}
  {:else}
    <!-- No tab selected state -->
    <div class="flex-1 flex items-center justify-center text-muted-foreground">
      <div class="text-center">
        <DatabaseIcon class="size-12 mx-auto mb-2 opacity-20" />
        <p class="text-sm">No ERD diagram selected</p>
      </div>
    </div>
  {/if}
</div>
