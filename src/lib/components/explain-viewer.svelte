<script lang="ts">
  import { useDatabase } from "$lib/hooks/database.svelte.js";
  import { SvelteFlow, Background, Controls, MiniMap } from "@xyflow/svelte";
  import "@xyflow/svelte/dist/style.css";
  import { Badge } from "$lib/components/ui/badge";
  import { Button } from "$lib/components/ui/button";
  import * as Collapsible from "$lib/components/ui/collapsible";
  import ClockIcon from "@lucide/svelte/icons/clock";
  import RowsIcon from "@lucide/svelte/icons/rows";
  import DatabaseIcon from "@lucide/svelte/icons/database";
  import FileCodeIcon from "@lucide/svelte/icons/file-code";
  import LoaderIcon from "@lucide/svelte/icons/loader";
  import FlameIcon from "@lucide/svelte/icons/flame";
  import ChevronDownIcon from "@lucide/svelte/icons/chevron-down";
  import ExplainPlanNode from "./explain-plan-node.svelte";
  import { layoutExplainPlan } from "$lib/utils/explain-layout";
  import { analyzeExplainPlan, type HotPathAnalysis } from "$lib/utils/explain-analysis";
  import type { Node, Edge, NodeTypes, ColorMode } from "@xyflow/svelte";
  import { mode } from "mode-watcher";
  import { m } from "$lib/paraglide/messages.js";

  let { tabId: propTabId = undefined }: { tabId?: string } = $props();

  const db = useDatabase();
  const activeExplainTab = $derived(
    propTabId
      ? db.state.explainTabs.find(t => t.id === propTabId) ?? null
      : db.state.activeExplainTab
  );

  // Map mode-watcher theme to xyflow colorMode
  const colorMode: ColorMode = $derived(mode.current === "dark" ? "dark" : "light");

  // Custom node types
  const nodeTypes: NodeTypes = {
    planNode: ExplainPlanNode,
  };

  // Analyze the explain result for hot paths
  const analysis: HotPathAnalysis | undefined = $derived.by(() => {
    if (!activeExplainTab?.result) {
      return undefined;
    }
    return analyzeExplainPlan(activeExplainTab.result);
  });

  // Convert explain result to xyflow nodes and edges
  const flowData = $derived.by(() => {
    if (!activeExplainTab?.result) {
      return { nodes: [] as Node[], edges: [] as Edge[] };
    }
    return layoutExplainPlan(activeExplainTab.result, analysis);
  });

  let nodes = $derived(flowData.nodes);
  let edges = $derived(flowData.edges);

  // State for bottleneck collapsible
  let bottlenecksOpen = $state(true);

  const handleViewQuery = () => {
    if (!activeExplainTab) return;
    db.queryTabs.focusOrCreate(activeExplainTab.sourceQuery, 'Query', () => db.ui.setActiveView("query"));
  };

  // Format time for display
  const formatTime = (ms: number) => {
    if (ms < 1) return `${(ms * 1000).toFixed(0)}µs`;
    if (ms < 1000) return `${ms.toFixed(2)}ms`;
    return `${(ms / 1000).toFixed(2)}s`;
  };
</script>

<div class="flex flex-col h-full">
  {#if activeExplainTab}
    {#if activeExplainTab.isExecuting}
      <!-- Loading state -->
      <div class="flex-1 flex items-center justify-center">
        <div class="flex flex-col items-center gap-3">
          <LoaderIcon class="size-8 animate-spin text-muted-foreground" />
          <p class="text-sm text-muted-foreground">{m.explain_analyzing()}</p>
        </div>
      </div>
    {:else if activeExplainTab.result}
      <!-- Summary Header -->
      <div class="p-4 border-b bg-muted/30 shrink-0">
        <div class="flex items-start justify-between">
          <div>
            <h2 class="text-lg font-semibold flex items-center gap-2">
              <DatabaseIcon class="size-5" />
              {m.explain_query_plan()}
              {#if activeExplainTab.result.isAnalyze}
                <Badge variant="default">{m.explain_analyzed()}</Badge>
              {/if}
            </h2>
            <div class="flex items-center gap-4 mt-2 text-sm text-muted-foreground">
              {#if activeExplainTab.result.planningTime > 0}
                <span class="flex items-center gap-1">
                  <ClockIcon class="size-3" />
                  {m.explain_planning()} {activeExplainTab.result.planningTime.toFixed(2)}ms
                </span>
              {/if}
              {#if activeExplainTab.result.executionTime !== undefined}
                <span class="flex items-center gap-1">
                  <ClockIcon class="size-3" />
                  {m.explain_execution()} {activeExplainTab.result.executionTime.toFixed(2)}ms
                </span>
              {/if}
              {#if activeExplainTab.result.plan.planRows !== undefined}
                <span class="flex items-center gap-1">
                  <RowsIcon class="size-3" />
                  {m.explain_estimated_rows()} {activeExplainTab.result.plan.planRows.toLocaleString()}
                </span>
              {/if}
            </div>
          </div>
          <Button size="sm" variant="outline" onclick={handleViewQuery}>
            <FileCodeIcon class="size-3 me-1" />
            {m.explain_view_query()}
          </Button>
        </div>

        <!-- Bottlenecks Summary -->
        {#if analysis?.hasAnalyzeData && analysis.bottlenecks.length > 0}
          <Collapsible.Root bind:open={bottlenecksOpen} class="mt-3">
            <Collapsible.Trigger class="flex items-center gap-2 text-sm font-medium text-muted-foreground hover:text-foreground transition-colors">
              <FlameIcon class="size-4 text-red-500" />
              <span>{m.explain_bottlenecks()} ({analysis.bottlenecks.length})</span>
              <ChevronDownIcon class="size-4 transition-transform {bottlenecksOpen ? 'rotate-180' : ''}" />
            </Collapsible.Trigger>
            <Collapsible.Content class="mt-2">
              <div class="flex flex-wrap gap-2">
                {#each analysis.bottlenecks as bottleneck}
                  <Badge
                    variant={bottleneck.tier === "critical" ? "destructive" : "outline"}
                    class="text-xs gap-1.5 {bottleneck.tier === 'warning' ? 'bg-orange-100 border-orange-400 text-orange-700 dark:bg-orange-950 dark:border-orange-700 dark:text-orange-300' : ''}"
                  >
                    {#if bottleneck.tier === "critical"}
                      <FlameIcon class="size-3" />
                    {/if}
                    <span class="font-mono">{bottleneck.nodeType}</span>
                    {#if bottleneck.relationName}
                      <span class="text-muted-foreground">({bottleneck.relationName})</span>
                    {/if}
                    <span class="font-semibold">{Math.round(bottleneck.percentageOfTotal * 100)}%</span>
                    <span class="text-muted-foreground">· {formatTime(bottleneck.effectiveTime)}</span>
                  </Badge>
                {/each}
              </div>
            </Collapsible.Content>
          </Collapsible.Root>
        {/if}
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
          nodesDraggable={false}
          nodesConnectable={false}
          elementsSelectable={false}
          deleteKey={null}
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
          <p class="text-sm">{m.explain_no_plan_available()}</p>
        </div>
      </div>
    {/if}
  {:else}
    <!-- No tab selected state -->
    <div class="flex-1 flex items-center justify-center text-muted-foreground">
      <div class="text-center">
        <DatabaseIcon class="size-12 mx-auto mb-2 opacity-20" />
        <p class="text-sm">{m.explain_no_plan_selected()}</p>
      </div>
    </div>
  {/if}
</div>
