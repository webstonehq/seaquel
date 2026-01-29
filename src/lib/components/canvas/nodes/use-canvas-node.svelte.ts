import { useDatabase } from '$lib/hooks/database.svelte.js';

/**
 * Shared logic for all canvas node types.
 * Provides common handlers for node removal and resizing.
 *
 * Accepts a getter to read `id` reactively and avoid the
 * `state_referenced_locally` warning from Svelte 5.
 */
export function useCanvasNode(getId: () => string) {
	const db = useDatabase();

	function handleRemove() {
		db.canvas.removeNode(getId());
	}

	function handleResizeEnd(_event: unknown, params: { width: number; height: number }) {
		db.canvas.updateNodeDimensions(getId(), params.width, params.height);
	}

	return {
		db,
		handleRemove,
		handleResizeEnd
	};
}
