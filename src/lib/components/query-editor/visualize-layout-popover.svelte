<script lang="ts">
	import { Button } from '$lib/components/ui/button';
	import * as Popover from '$lib/components/ui/popover';
	import { Label } from '$lib/components/ui/label';
	import { Input } from '$lib/components/ui/input';
	import { SettingsIcon, ArrowDownIcon, ArrowUpIcon, ArrowRightIcon, ArrowLeftIcon } from '@lucide/svelte';
	import { DEFAULT_LAYOUT_OPTIONS, type QueryLayoutOptions, type LayoutDirection } from '$lib/utils/query-visual-layout';

	type Props = {
		layoutOptions: QueryLayoutOptions;
		onLayoutChange: (options: QueryLayoutOptions) => void;
	};

	let { layoutOptions, onLayoutChange }: Props = $props();

	const layoutDirections: { value: LayoutDirection; label: string; icon: typeof ArrowDownIcon }[] = [
		{ value: 'TB', label: 'Top to Bottom', icon: ArrowDownIcon },
		{ value: 'BT', label: 'Bottom to Top', icon: ArrowUpIcon },
		{ value: 'LR', label: 'Left to Right', icon: ArrowRightIcon },
		{ value: 'RL', label: 'Right to Left', icon: ArrowLeftIcon }
	];

	const setDirection = (dir: LayoutDirection) => {
		onLayoutChange({ ...layoutOptions, direction: dir });
	};

	const resetLayout = () => {
		onLayoutChange({ ...DEFAULT_LAYOUT_OPTIONS });
	};
</script>

<Popover.Root>
	<Popover.Trigger>
		<Button variant="ghost" size="sm" class="h-7 gap-1.5 px-2">
			<SettingsIcon class="size-3.5" />
			Layout
		</Button>
	</Popover.Trigger>
	<Popover.Content class="w-64" align="end">
		<div class="space-y-4">
			<div class="flex items-center justify-between">
				<h4 class="font-medium text-sm">Layout Settings</h4>
				<Button variant="ghost" size="sm" class="h-6 text-xs" onclick={resetLayout}>
					Reset
				</Button>
			</div>
			<div class="space-y-2">
				<Label class="text-xs text-muted-foreground">Direction</Label>
				<div class="grid grid-cols-4 gap-1">
					{#each layoutDirections as dir}
						<Button
							variant={layoutOptions.direction === dir.value ? 'default' : 'outline'}
							size="sm"
							class="h-8 px-2"
							onclick={() => setDirection(dir.value)}
							title={dir.label}
						>
							<dir.icon class="size-4" />
						</Button>
					{/each}
				</div>
			</div>
			<div class="space-y-2">
				<Label class="text-xs text-muted-foreground">Node Spacing (px)</Label>
				<Input
					type="number"
					min={20}
					max={200}
					step={10}
					value={layoutOptions.nodeSpacing}
					oninput={(e) => {
						const val = parseInt(e.currentTarget.value, 10);
						if (!isNaN(val) && val >= 20 && val <= 200) {
							onLayoutChange({ ...layoutOptions, nodeSpacing: val });
						}
					}}
					class="h-8"
				/>
			</div>
			<div class="space-y-2">
				<Label class="text-xs text-muted-foreground">Level Spacing (px)</Label>
				<Input
					type="number"
					min={40}
					max={300}
					step={10}
					value={layoutOptions.rankSpacing}
					oninput={(e) => {
						const val = parseInt(e.currentTarget.value, 10);
						if (!isNaN(val) && val >= 40 && val <= 300) {
							onLayoutChange({ ...layoutOptions, rankSpacing: val });
						}
					}}
					class="h-8"
				/>
			</div>
		</div>
	</Popover.Content>
</Popover.Root>
