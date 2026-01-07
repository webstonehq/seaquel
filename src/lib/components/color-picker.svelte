<script lang="ts">
	import { oklchToHex, hexToOklch } from "$lib/themes/color-utils";
	import { Input } from "$lib/components/ui/input";

	interface Props {
		value: string;
		label: string;
		onchange: (value: string) => void;
	}

	let { value, label, onchange }: Props = $props();

	// Convert oklch to hex for color picker
	const hexValue = $derived(oklchToHex(value));

	function handleColorChange(e: Event) {
		const target = e.target as HTMLInputElement;
		const newOklch = hexToOklch(target.value);
		onchange(newOklch);
	}

	function handleOklchChange(e: Event) {
		const target = e.target as HTMLInputElement;
		onchange(target.value);
	}
</script>

<div class="flex items-center gap-2">
	<label class="flex items-center gap-2 cursor-pointer">
		<input
			type="color"
			value={hexValue}
			onchange={handleColorChange}
			class="w-8 h-8 rounded border cursor-pointer"
		/>
		<span class="text-sm min-w-[100px]">{label}</span>
	</label>
	<Input
		type="text"
		value={value}
		onchange={handleOklchChange}
		class="font-mono text-xs h-8 flex-1"
		placeholder="oklch(0.5 0.2 180)"
	/>
</div>
