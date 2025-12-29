import {
	Virtualizer,
	elementScroll,
	observeElementOffset,
	observeElementRect,
	type VirtualizerOptions,
	type VirtualItem
} from '@tanstack/virtual-core';

export type { VirtualItem };

export interface UseVirtualizerOptions<TScrollElement extends Element> {
	count: number;
	getScrollElement: () => TScrollElement | null | undefined;
	estimateSize: () => number;
	overscan?: number;
}

/**
 * Svelte 5 adapter for TanStack Virtual.
 * Uses runes ($state, $effect) instead of Svelte stores.
 */
export function useVirtualizer<TScrollElement extends Element>(
	getOptions: () => UseVirtualizerOptions<TScrollElement>
) {
	// Reactive state to trigger re-renders
	let _virtualItems = $state<VirtualItem[]>([]);
	let _totalSize = $state(0);

	let virtualizer: Virtualizer<TScrollElement, Element> | null = null;
	let cleanup: (() => void) | undefined;
	let lastScrollElement: TScrollElement | null = null;

	$effect(() => {
		const options = getOptions();
		const scrollElement = options.getScrollElement();

		// Don't initialize until we have a scroll element
		if (!scrollElement) {
			_virtualItems = [];
			_totalSize = 0;
			return;
		}

		// If scroll element changed, recreate virtualizer
		if (scrollElement !== lastScrollElement) {
			cleanup?.();
			virtualizer = null;
			lastScrollElement = scrollElement;
		}

		const resolvedOptions: VirtualizerOptions<TScrollElement, Element> = {
			count: options.count,
			getScrollElement: () => scrollElement,
			estimateSize: options.estimateSize,
			overscan: options.overscan ?? 10,
			observeElementRect,
			observeElementOffset,
			scrollToFn: elementScroll,
			onChange: (instance) => {
				// Update reactive state to trigger re-renders
				_virtualItems = instance.getVirtualItems();
				_totalSize = instance.getTotalSize();
			}
		};

		if (!virtualizer) {
			virtualizer = new Virtualizer(resolvedOptions);
			cleanup = virtualizer._didMount();
			// Initial state
			_virtualItems = virtualizer.getVirtualItems();
			_totalSize = virtualizer.getTotalSize();
		} else {
			virtualizer.setOptions(resolvedOptions);
			virtualizer._willUpdate();
			// Update state after options change
			_virtualItems = virtualizer.getVirtualItems();
			_totalSize = virtualizer.getTotalSize();
		}

		return () => {
			cleanup?.();
			virtualizer = null;
		};
	});

	return {
		get virtualItems(): VirtualItem[] {
			return _virtualItems;
		},
		get totalSize(): number {
			return _totalSize;
		},
		scrollToIndex(index: number, options?: { align?: 'start' | 'center' | 'end' | 'auto' }) {
			virtualizer?.scrollToIndex(index, options);
		},
		scrollToOffset(offset: number, options?: { align?: 'start' | 'center' | 'end' | 'auto' }) {
			virtualizer?.scrollToOffset(offset, options);
		},
		measure() {
			virtualizer?.measure();
		}
	};
}
