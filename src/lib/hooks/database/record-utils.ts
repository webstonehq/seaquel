/**
 * Helper functions for updating Record-based state with Svelte 5 reactivity.
 * Records work with simple spread syntax, making updates more ergonomic than Maps.
 *
 * Note: Most updates can be done directly with spread syntax:
 *   state.data = { ...state.data, [key]: value }
 *
 * These helpers are provided for common patterns that benefit from abstraction.
 */

/**
 * Update an item in an array stored in a Record.
 * Creates new objects for proper reactivity.
 *
 * @example
 * state.tabsByConnection = updateRecordArrayItem(
 *   state.tabsByConnection,
 *   connectionId,
 *   tabId,
 *   { name: 'New Name' }
 * );
 */
export function updateRecordArrayItem<T extends { id: string }>(
	record: Record<string, T[]>,
	key: string,
	itemId: string,
	updates: Partial<T>
): Record<string, T[]> {
	const currentArray = record[key] ?? [];
	const newArray = currentArray.map((item) =>
		item.id === itemId ? { ...item, ...updates } : item
	);
	return { ...record, [key]: newArray };
}

/**
 * Remove an item from an array stored in a Record.
 *
 * @example
 * state.tabsByConnection = removeRecordArrayItem(
 *   state.tabsByConnection,
 *   connectionId,
 *   tabId
 * );
 */
export function removeRecordArrayItem<T extends { id: string }>(
	record: Record<string, T[]>,
	key: string,
	itemId: string
): Record<string, T[]> {
	const currentArray = record[key] ?? [];
	const newArray = currentArray.filter((item) => item.id !== itemId);
	return { ...record, [key]: newArray };
}

/**
 * Add an item to an array stored in a Record.
 *
 * @example
 * state.tabsByConnection = addRecordArrayItem(
 *   state.tabsByConnection,
 *   connectionId,
 *   newTab
 * );
 */
export function addRecordArrayItem<T>(
	record: Record<string, T[]>,
	key: string,
	item: T
): Record<string, T[]> {
	const currentArray = record[key] ?? [];
	return { ...record, [key]: [...currentArray, item] };
}

/**
 * Delete a key from a Record.
 *
 * @example
 * state.tabsByConnection = deleteRecordKey(state.tabsByConnection, connectionId);
 */
export function deleteRecordKey<T>(record: Record<string, T>, key: string): Record<string, T> {
	const { [key]: _, ...rest } = record;
	return rest;
}
