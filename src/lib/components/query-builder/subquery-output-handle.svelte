<script lang="ts">
	/**
	 * Output handle component for subqueries.
	 * Used to visually indicate that a subquery can be connected to outer query elements.
	 *
	 * This component wraps the standard SvelteFlow Handle with custom styling
	 * for subquery-specific connections.
	 */
	import { Handle, Position } from '@xyflow/svelte';

	interface Props {
		/** Unique identifier for this handle */
		id?: string;
		/** Whether connections can be made to/from this handle */
		isConnectable?: boolean;
		/** Visual style variant */
		variant?: 'default' | 'where' | 'from' | 'select';
	}

	let { id = 'subquery-output', isConnectable = true, variant = 'default' }: Props = $props();

	const VARIANT_COLORS = {
		default: '!bg-primary',
		where: '!bg-indigo-500',
		from: '!bg-emerald-500',
		select: '!bg-amber-500'
	};
</script>

<Handle
	type="source"
	position={Position.Right}
	{id}
	{isConnectable}
	class="!w-3 !h-3 {VARIANT_COLORS[variant]} !border-2 !border-background"
/>
