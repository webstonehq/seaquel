<script lang="ts">
    import {
        BaseEdge,
        EdgeLabel,
        getBezierPath,
        type Position,
    } from "@xyflow/svelte";
    import type { JoinType } from "$lib/types";
    import { useQueryBuilder } from "$lib/hooks/query-builder.svelte.js";
    import * as DropdownMenu from "$lib/components/ui/dropdown-menu";
    import * as Tooltip from "$lib/components/ui/tooltip";
    import ChevronDownIcon from "@lucide/svelte/icons/chevron-down";
    import CheckIcon from "@lucide/svelte/icons/check";

    interface Props {
        id: string;
        sourceX: number;
        sourceY: number;
        targetX: number;
        targetY: number;
        sourcePosition: Position;
        targetPosition: Position;
        data?: {
            joinId: string;
            joinType: JoinType;
        };
        markerEnd?: string;
        style?: string;
    }

    let {
        id,
        sourceX,
        sourceY,
        targetX,
        targetY,
        sourcePosition,
        targetPosition,
        data,
        markerEnd,
        style,
    }: Props = $props();

    const queryBuilder = useQueryBuilder();

    // Join type options with short labels for the pill
    const joinOptions: Array<{
        type: JoinType;
        short: string;
        label: string;
        description: string;
    }> = [
        {
            type: "INNER",
            short: "INNER",
            label: "Inner Join",
            description: "Only matching rows",
        },
        {
            type: "LEFT",
            short: "LEFT",
            label: "Left Join",
            description: "All left + matching right",
        },
        {
            type: "RIGHT",
            short: "RIGHT",
            label: "Right Join",
            description: "All right + matching left",
        },
        {
            type: "FULL",
            short: "FULL",
            label: "Full Outer",
            description: "All rows from both",
        },
    ];

    // Compute the bezier path and label position
    const pathData = $derived(
        getBezierPath({
            sourceX,
            sourceY,
            sourcePosition,
            targetX,
            targetY,
            targetPosition,
        }),
    );

    const edgePath = $derived(pathData[0]);
    const labelX = $derived(pathData[1]);
    const labelY = $derived(pathData[2]);

    // Current join type
    const joinType = $derived(data?.joinType ?? "INNER");
    const currentOption = $derived(
        joinOptions.find((opt) => opt.type === joinType) ?? joinOptions[0],
    );

    // Determine if each end should be filled (all rows) or hollow (matching only)
    // Filled = all rows from that table, Hollow = only matching rows
    const sourceIsFilled = $derived(
        joinType === "LEFT" || joinType === "FULL"
    );
    const targetIsFilled = $derived(
        joinType === "RIGHT" || joinType === "FULL"
    );

    const circleRadius = 5;

    function handleJoinTypeChange(newType: JoinType) {
        if (data?.joinId) {
            queryBuilder.updateJoinType(data.joinId, newType);
        }
    }

    // Track dropdown state to hide tooltip when open
    let dropdownOpen = $state(false);
</script>

<BaseEdge path={edgePath} {markerEnd} {style} />

<!-- Connection point indicators: filled = all rows, hollow = matching only -->
<circle
    cx={sourceX}
    cy={sourceY}
    r={circleRadius}
    class={sourceIsFilled
        ? "fill-primary stroke-primary"
        : "fill-background stroke-muted-foreground"}
    stroke-width="2"
/>
<circle
    cx={targetX}
    cy={targetY}
    r={circleRadius}
    class={targetIsFilled
        ? "fill-primary stroke-primary"
        : "fill-background stroke-muted-foreground"}
    stroke-width="2"
/>

<EdgeLabel x={labelX} y={labelY} class="nodrag nopan pointer-events-auto !bg-transparent">
    <Tooltip.Root open={dropdownOpen ? false : undefined}>
        <Tooltip.Trigger>
            <DropdownMenu.Root bind:open={dropdownOpen}>
                <DropdownMenu.Trigger
                    class="flex items-center gap-1 px-2 py-1 rounded-md bg-popover text-popover-foreground border border-border text-xs shadow-md hover:bg-accent transition-colors"
                >
                    {@render JoinIcon({ type: joinType, class: "size-3.5 opacity-70" })}
                    <span class="font-medium">{currentOption.short}</span>
                    <ChevronDownIcon class="size-3 opacity-50" />
                </DropdownMenu.Trigger>
                <DropdownMenu.Content align="center" class="w-44">
                    {#each joinOptions as option (option.type)}
                        <DropdownMenu.Item
                            onclick={() => handleJoinTypeChange(option.type)}
                            class="flex items-center gap-2 cursor-pointer"
                        >
                            {@render JoinIcon({
                                type: option.type,
                                class: "size-4 shrink-0",
                            })}
                            <div class="flex-1 min-w-0">
                                <div class="font-medium text-sm">{option.label}</div>
                                <div class="text-xs text-muted-foreground truncate">
                                    {option.description}
                                </div>
                            </div>
                            {#if option.type === joinType}
                                <CheckIcon class="size-4 shrink-0 text-primary" />
                            {/if}
                        </DropdownMenu.Item>
                    {/each}
                </DropdownMenu.Content>
            </DropdownMenu.Root>
        </Tooltip.Trigger>
        <Tooltip.Content>
            <div class="flex items-center gap-2">
                {@render JoinIcon({ type: joinType, class: "size-4 shrink-0" })}
                <div>
                    <div class="font-medium">{currentOption.label}</div>
                    <div class="text-xs text-muted-foreground">{currentOption.description}</div>
                </div>
            </div>
        </Tooltip.Content>
    </Tooltip.Root>
</EdgeLabel>

<!-- Join type Venn diagram icons -->
{#snippet JoinIcon({
    type,
    class: className,
}: {
    type: JoinType;
    class?: string;
})}
    <svg
        viewBox="0 0 24 24"
        class={className}
        fill="none"
        stroke="currentColor"
        stroke-width="1.5"
    >
        {#if type === "INNER"}
            <!-- Inner: only intersection filled -->
            <circle cx="9" cy="12" r="6" class="opacity-30" />
            <circle cx="15" cy="12" r="6" class="opacity-30" />
            <path
                d="M12 7.5a6 6 0 0 1 0 9 6 6 0 0 1 0-9z"
                fill="currentColor"
                stroke="none"
            />
        {:else if type === "LEFT"}
            <!-- Left: left circle filled -->
            <circle
                cx="9"
                cy="12"
                r="6"
                fill="currentColor"
                class="opacity-60"
            />
            <circle cx="15" cy="12" r="6" class="opacity-30" />
        {:else if type === "RIGHT"}
            <!-- Right: right circle filled -->
            <circle cx="9" cy="12" r="6" class="opacity-30" />
            <circle
                cx="15"
                cy="12"
                r="6"
                fill="currentColor"
                class="opacity-60"
            />
        {:else if type === "FULL"}
            <!-- Full: both circles filled -->
            <circle
                cx="9"
                cy="12"
                r="6"
                fill="currentColor"
                class="opacity-60"
            />
            <circle
                cx="15"
                cy="12"
                r="6"
                fill="currentColor"
                class="opacity-60"
            />
        {/if}
    </svg>
{/snippet}
