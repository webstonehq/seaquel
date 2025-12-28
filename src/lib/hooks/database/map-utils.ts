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
