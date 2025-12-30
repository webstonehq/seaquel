/**
 * Helper functions for updating Map state with Svelte 5 reactivity.
 * Maps need to be replaced with new instances to trigger reactivity.
 */

/**
 * Set a value in a Map state while maintaining reactivity.
 * Creates a new Map instance and calls the setter.
 */
export function setMapValue<K, V>(
  getter: () => Map<K, V>,
  setter: (m: Map<K, V>) => void,
  key: K,
  value: V
): void {
  const newMap = new Map(getter());
  newMap.set(key, value);
  setter(newMap);
}

/**
 * Delete a key from a Map state while maintaining reactivity.
 * Creates a new Map instance and calls the setter.
 */
export function deleteMapKey<K, V>(
  getter: () => Map<K, V>,
  setter: (m: Map<K, V>) => void,
  key: K
): void {
  const newMap = new Map(getter());
  newMap.delete(key);
  setter(newMap);
}

/**
 * Update a specific item in an array stored in a Map while maintaining reactivity.
 * Creates new Map and array instances, and replaces the item with updated properties.
 */
export function updateMapArrayItem<K, T extends { id: string }>(
  getter: () => Map<K, T[]>,
  setter: (m: Map<K, T[]>) => void,
  mapKey: K,
  itemId: string,
  updates: Partial<T>
): void {
  const currentMap = getter();
  const currentArray = currentMap.get(mapKey) || [];

  // Create new array with updated item
  const newArray = currentArray.map(item =>
    item.id === itemId ? { ...item, ...updates } : item
  );

  // Create new Map with new array
  const newMap = new Map(currentMap);
  newMap.set(mapKey, newArray);
  setter(newMap);
}
