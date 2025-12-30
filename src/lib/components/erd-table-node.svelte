<script lang="ts">
  import { Handle, Position } from "@xyflow/svelte";
  import type { SchemaTable } from "$lib/types";
  import KeyIcon from "@lucide/svelte/icons/key";
  import LinkIcon from "@lucide/svelte/icons/link";
  import { m } from "$lib/paraglide/messages.js";

  interface Props {
    data: {
      table: SchemaTable;
      width: number;
      height: number;
    };
    isConnectable: boolean;
  }

  let { data, isConnectable }: Props = $props();

  // Abbreviate data types for display
  const abbreviateType = (type: string): string => {
    const abbrevMap: Record<string, string> = {
      "character varying": "varchar",
      "timestamp without time zone": "timestamp",
      "timestamp with time zone": "timestamptz",
      "double precision": "double",
      "boolean": "bool",
      "integer": "int",
      "bigint": "bigint",
      "smallint": "smallint",
      "text": "text",
      "uuid": "uuid",
      "json": "json",
      "jsonb": "jsonb",
      "bytea": "bytea",
      "date": "date",
      "time": "time",
      "numeric": "numeric",
      "real": "real",
      "serial": "serial",
      "bigserial": "bigserial",
    };

    const lowerType = type.toLowerCase();
    for (const [full, abbrev] of Object.entries(abbrevMap)) {
      if (lowerType.startsWith(full)) {
        return abbrev;
      }
    }

    // Truncate long types
    if (type.length > 12) {
      return type.substring(0, 10) + "...";
    }
    return type;
  };

  const maxVisibleColumns = 15;
  const visibleColumns = $derived(data.table.columns.slice(0, maxVisibleColumns));
  const hiddenCount = $derived(data.table.columns.length - maxVisibleColumns);
</script>

<div
  class="bg-card border-2 border-border rounded-lg shadow-md overflow-hidden relative"
  style="width: {data.width}px;"
>
  <!-- Hidden handles for edge connections -->
  <Handle
    type="target"
    position={Position.Left}
    {isConnectable}
    class="!opacity-0 !w-0 !h-0"
  />
  <Handle
    type="source"
    position={Position.Right}
    {isConnectable}
    class="!opacity-0 !w-0 !h-0"
  />

  <!-- Table Header -->
  <div class="bg-primary text-primary-foreground px-3 py-2 font-medium text-sm flex items-center justify-between">
    <span class="truncate" title="{data.table.schema}.{data.table.name}">
      {data.table.name}
    </span>
    <span class="text-xs opacity-70 shrink-0 ml-2">{data.table.schema}</span>
  </div>

  <!-- Columns List -->
  <div class="text-xs divide-y divide-border">
    {#each visibleColumns as column (column.name)}
      <div class="px-3 py-1 flex items-center gap-2 hover:bg-muted/50">
        <!-- Column icons -->
        <div class="flex items-center gap-1 shrink-0 w-8">
          {#if column.isPrimaryKey}
            <KeyIcon class="size-3 text-amber-500" />
          {/if}
          {#if column.isForeignKey}
            <LinkIcon class="size-3 text-blue-500" />
          {/if}
        </div>

        <!-- Column name -->
        <span class="flex-1 truncate font-mono" title={column.name}>
          {column.name}
        </span>

        <!-- Column type -->
        <span class="text-muted-foreground shrink-0" title={column.type}>
          {abbreviateType(column.type)}
        </span>

        <!-- Nullable indicator -->
        {#if column.nullable}
          <span class="text-muted-foreground/50 shrink-0">?</span>
        {/if}
      </div>
    {/each}

    {#if hiddenCount > 0}
      <div class="px-3 py-1 text-center text-muted-foreground italic">
        {m.erd_more_columns({ count: hiddenCount })}
      </div>
    {/if}
  </div>
</div>
