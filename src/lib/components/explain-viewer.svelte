<script lang="ts">
  import { useDatabase } from "$lib/hooks/database.svelte.js";
  import { SvelteFlow, Background, Controls, MiniMap } from "@xyflow/svelte";
  import "@xyflow/svelte/dist/style.css";
  import { Badge } from "$lib/components/ui/badge";
  import { Button } from "$lib/components/ui/button";
  import { ClockIcon, RowsIcon, DatabaseIcon, FileCodeIcon, LoaderIcon } from "@lucide/svelte";
  import ExplainPlanNode from "./explain-plan-node.svelte";
  import { layoutExplainPlan } from "$lib/utils/explain-layout";
  import type { Node, Edge, NodeTypes, ColorMode } from "@xyflow/svelte";
  import { mode } from "mode-watcher";

  const db = useDatabase();

  // Map mode-watcher theme to xyflow colorMode
  const colorMode: ColorMode = $derived(mode.current === "dark" ? "dark" : "light");

  // Custom node types
  const nodeTypes: NodeTypes = {
    planNode: ExplainPlanNode,
  };

  // Convert explain result to xyflow nodes and edges
  const flowData = $derived.by(() => {
    if (!db.activeExplainTab?.result) {
      return { nodes: [] as Node[], edges: [] as Edge[] };
    }
    return layoutExplainPlan(db.activeExplainTab.result);
  });

  let nodes = $derived(flowData.nodes);
  let edges = $derived(flowData.edges);

  const handleViewQuery = () => {
    if (!db.activeExplainTab) return;
    db.addQueryTab(
      `Query`,
      db.activeExplainTab.sourceQuery
    );
    db.setActiveView("query");
  };
</script>

<div class="flex flex-col h-full">
  {#if db.activeExplainTab}
    {#if db.activeExplainTab.isExecuting}
      <!-- Loading state -->
      <div class="flex-1 flex items-center justify-center">
        <div class="flex flex-col items-center gap-3">
          <LoaderIcon class="size-8 animate-spin text-muted-foreground" />
          <p class="text-sm text-muted-foreground">Analyzing query plan...</p>
        </div>
      </div>
    {:else if db.activeExplainTab.result}
      <!-- Summary Header -->
      <div class="p-4 border-b bg-muted/30 shrink-0">
        <div class="flex items-start justify-between">
          <div>
            <h2 class="text-lg font-semibold flex items-center gap-2">
              <DatabaseIcon class="size-5" />
              Query Plan
              {#if db.activeExplainTab.result.isAnalyze}
                <Badge variant="default">Analyzed</Badge>
              {/if}
            </h2>
            <div class="flex items-center gap-4 mt-2 text-sm text-muted-foreground">
              <span class="flex items-center gap-1">
                <ClockIcon class="size-3" />
                Planning: {db.activeExplainTab.result.planningTime.toFixed(2)}ms
              </span>
              {#if db.activeExplainTab.result.executionTime !== undefined}
                <span class="flex items-center gap-1">
                  <ClockIcon class="size-3" />
                  Execution: {db.activeExplainTab.result.executionTime.toFixed(2)}ms
                </span>
              {/if}
              <span class="flex items-center gap-1">
                <RowsIcon class="size-3" />
                Est. Rows: {db.activeExplainTab.result.plan.planRows.toLocaleString()}
              </span>
            </div>
          </div>
          <Button size="sm" variant="outline" onclick={handleViewQuery}>
            <FileCodeIcon class="size-3 mr-1" />
            View Query
          </Button>
        </div>
      </div>

      <!-- Flow Diagram -->
      <div class="flex-1 min-h-0">
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
          proOptions={{ hideAttribution: true }}
        >
          <Background />
          <Controls />
          <MiniMap />
        </SvelteFlow>
      </div>
    {:else}
      <!-- No result state -->
      <div class="flex-1 flex items-center justify-center text-muted-foreground">
        <div class="text-center">
          <DatabaseIcon class="size-12 mx-auto mb-2 opacity-20" />
          <p class="text-sm">No explain plan available</p>
        </div>
      </div>
    {/if}
  {:else}
    <!-- No tab selected state -->
    <div class="flex-1 flex items-center justify-center text-muted-foreground">
      <div class="text-center">
        <DatabaseIcon class="size-12 mx-auto mb-2 opacity-20" />
        <p class="text-sm">No explain plan selected</p>
      </div>
    </div>
  {/if}
</div>
