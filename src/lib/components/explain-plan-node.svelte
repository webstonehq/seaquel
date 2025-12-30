<script lang="ts">
  import { Handle, Position } from "@xyflow/svelte";
  import { Badge } from "$lib/components/ui/badge";
  import type { ExplainPlanNode } from "$lib/types";
  import { m } from "$lib/paraglide/messages.js";

  interface Props {
    data: ExplainPlanNode;
    isConnectable: boolean;
  }

  let { data, isConnectable }: Props = $props();

  // Calculate color based on cost (relative scale)
  const getCostColor = (cost: number) => {
    if (cost > 10000) return "bg-red-100 border-red-300 dark:bg-red-950 dark:border-red-800";
    if (cost > 1000) return "bg-orange-100 border-orange-300 dark:bg-orange-950 dark:border-orange-800";
    if (cost > 100) return "bg-yellow-100 border-yellow-300 dark:bg-yellow-950 dark:border-yellow-800";
    return "bg-green-100 border-green-300 dark:bg-green-950 dark:border-green-800";
  };

  // Format time nicely
  const formatTime = (ms: number | undefined) => {
    if (ms === undefined) return null;
    if (ms < 1) return `${(ms * 1000).toFixed(0)}us`;
    if (ms < 1000) return `${ms.toFixed(2)}ms`;
    return `${(ms / 1000).toFixed(2)}s`;
  };

  // Get node type badge color
  const getNodeTypeBadge = (nodeType: string): "default" | "secondary" | "outline" | "destructive" => {
    if (nodeType.includes("Scan")) return "secondary";
    if (nodeType.includes("Join")) return "default";
    if (nodeType.includes("Sort")) return "outline";
    if (nodeType.includes("Aggregate")) return "default";
    return "outline";
  };
</script>

<div class={`px-4 py-3 rounded-lg border-2 shadow-sm min-w-[250px] max-w-[280px] ${getCostColor(data.totalCost)}`}>
  <!-- Input handle (top) -->
  <Handle type="target" position={Position.Top} {isConnectable} class="!bg-muted-foreground" />

  <!-- Node Type Header -->
  <div class="flex items-center justify-between mb-2 gap-2">
    <Badge variant={getNodeTypeBadge(data.nodeType)} class="text-xs font-medium shrink-0">
      {data.nodeType}
    </Badge>
    {#if data.joinType}
      <Badge variant="outline" class="text-xs">{data.joinType}</Badge>
    {/if}
  </div>

  <!-- Table/Index Name -->
  {#if data.relationName}
    <div class="text-sm font-mono font-medium mb-2 truncate" title={data.relationName}>
      {data.relationName}
      {#if data.alias && data.alias !== data.relationName}
        <span class="text-muted-foreground"> as {data.alias}</span>
      {/if}
    </div>
  {/if}

  {#if data.indexName}
    <div class="text-xs text-muted-foreground mb-2 truncate" title={data.indexName}>
      {m.explain_index()} <span class="font-mono">{data.indexName}</span>
    </div>
  {/if}

  <!-- Cost & Rows -->
  <div class="grid grid-cols-2 gap-2 text-xs">
    <div>
      <span class="text-muted-foreground">{m.explain_cost()}</span>
      <span class="font-mono ms-1">{data.totalCost.toFixed(2)}</span>
    </div>
    <div>
      <span class="text-muted-foreground">{m.explain_rows()}</span>
      <span class="font-mono ms-1">{data.planRows.toLocaleString()}</span>
    </div>
  </div>

  <!-- Actual metrics (ANALYZE only) -->
  {#if data.actualTotalTime !== undefined}
    <div class="border-t mt-2 pt-2 grid grid-cols-2 gap-2 text-xs">
      <div>
        <span class="text-muted-foreground">{m.explain_time()}</span>
        <span class="font-mono ms-1 text-blue-600 dark:text-blue-400">
          {formatTime(data.actualTotalTime)}
        </span>
      </div>
      <div>
        <span class="text-muted-foreground">{m.explain_actual()}</span>
        <span class="font-mono ms-1 text-blue-600 dark:text-blue-400">
          {data.actualRows?.toLocaleString()}
        </span>
      </div>
      {#if data.actualLoops && data.actualLoops > 1}
        <div class="col-span-2">
          <span class="text-muted-foreground">{m.explain_loops()}</span>
          <span class="font-mono ms-1">{data.actualLoops}</span>
        </div>
      {/if}
    </div>
  {/if}

  <!-- Filter/Conditions -->
  {#if data.filter || data.indexCond || data.hashCond}
    <div class="border-t mt-2 pt-2 text-xs space-y-1">
      {#if data.filter}
        <div class="text-muted-foreground truncate" title={data.filter}>
          {m.explain_filter()} <span class="font-mono">{data.filter}</span>
        </div>
      {/if}
      {#if data.indexCond}
        <div class="text-muted-foreground truncate" title={data.indexCond}>
          {m.explain_index_cond()} <span class="font-mono">{data.indexCond}</span>
        </div>
      {/if}
      {#if data.hashCond}
        <div class="text-muted-foreground truncate" title={data.hashCond}>
          {m.explain_hash_cond()} <span class="font-mono">{data.hashCond}</span>
        </div>
      {/if}
    </div>
  {/if}

  <!-- Output handle (bottom) -->
  <Handle type="source" position={Position.Bottom} {isConnectable} class="!bg-muted-foreground" />
</div>
