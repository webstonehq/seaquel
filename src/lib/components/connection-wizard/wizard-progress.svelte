<script lang="ts">
	interface Props {
		currentStep: number;
		totalSteps: number;
	}

	let { currentStep, totalSteps }: Props = $props();
</script>

{#if totalSteps > 0}
	<div class="flex items-center justify-center gap-2">
		{#each Array(totalSteps) as _, index}
			<div
				class="h-1.5 rounded-full transition-all duration-300 {index < currentStep
					? 'bg-primary w-6'
					: index === currentStep
						? 'bg-primary w-8'
						: 'bg-muted w-6'}"
			></div>
		{/each}
	</div>
{/if}
