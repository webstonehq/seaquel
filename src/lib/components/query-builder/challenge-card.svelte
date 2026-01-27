<script lang="ts">
	import type { Challenge, ChallengeCriterion } from '$lib/types';
	import { useQueryBuilder } from '$lib/hooks/query-builder.svelte';
	import { Button } from '$lib/components/ui/button';
	import { Card, CardHeader, CardTitle, CardContent } from '$lib/components/ui/card';
	import { Badge } from '$lib/components/ui/badge';
	import CheckCircle2Icon from '@lucide/svelte/icons/check-circle-2';
	import CircleIcon from '@lucide/svelte/icons/circle';
	import LightbulbIcon from '@lucide/svelte/icons/lightbulb';
	import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
	import ChevronLeftIcon from '@lucide/svelte/icons/chevron-left';
	import SkipForwardIcon from '@lucide/svelte/icons/skip-forward';

	interface Props {
		challenge: Challenge;
		challengeIndex: number;
		totalChallenges: number;
		onComplete?: () => void;
		onSkip?: () => void;
		onPrevious?: () => void;
		onGoTo?: (index: number) => void;
		completedChallenges?: Set<string>;
		allChallenges?: { id: string }[];
	}

	let { challenge, challengeIndex, totalChallenges, onComplete, onSkip, onPrevious, onGoTo, completedChallenges, allChallenges }: Props = $props();

	const hasPrevious = $derived(challengeIndex > 0);
	const hasNext = $derived(challengeIndex < totalChallenges - 1);

	const qb = useQueryBuilder();

	let showHint = $state(false);

	// Reset hint visibility when challenge changes
	$effect(() => {
		challengeIndex;
		showHint = false;
	});

	// The SQL to validate - use custom SQL if user has typed something, otherwise use generated SQL
	const activeSql = $derived(qb.customSql ?? qb.generatedSql);

	// Evaluate criteria against current state
	const evaluatedCriteria = $derived<ChallengeCriterion[]>(
		challenge.criteria.map((c) => ({
			...c,
			satisfied: c.check(qb.snapshot, activeSql)
		}))
	);

	const isComplete = $derived(evaluatedCriteria.every((c) => c.satisfied));
	const completedCount = $derived(evaluatedCriteria.filter((c) => c.satisfied).length);
</script>

<Card class="border-primary/20">
	<CardHeader class="pb-2">
		<div class="flex items-center justify-between">
			<!-- Challenge indicator dots -->
			{#if allChallenges && onGoTo}
				<div class="flex items-center gap-1">
					{#each allChallenges as ch, i (ch.id)}
						<button
							type="button"
							class={[
								"size-2 rounded-full transition-colors",
								i === challengeIndex
									? "bg-primary ring-2 ring-primary/30"
									: completedChallenges?.has(ch.id)
										? "bg-green-500 hover:bg-green-400"
										: "bg-muted-foreground/30 hover:bg-muted-foreground/50"
							]}
							onclick={() => onGoTo?.(i)}
							title={`Challenge ${i + 1}`}
						></button>
					{/each}
				</div>
			{:else}
				<Badge variant="secondary">
					Challenge {challengeIndex + 1} of {totalChallenges}
				</Badge>
			{/if}
			{#if isComplete}
				<Badge class="bg-green-500 text-white">Complete!</Badge>
			{:else}
				<Badge variant="outline">
					{completedCount}/{evaluatedCriteria.length}
				</Badge>
			{/if}
		</div>
		<CardTitle class="text-base">{challenge.title}</CardTitle>
	</CardHeader>

	<CardContent class="space-y-4">
		<p class="text-sm text-muted-foreground">{challenge.description}</p>

		<!-- Criteria checklist -->
		<div class="space-y-2">
			{#each evaluatedCriteria as criterion (criterion.id)}
				<div class="flex items-center gap-2 text-sm">
					{#if criterion.satisfied}
						<CheckCircle2Icon class="size-4 text-green-500 shrink-0" />
					{:else}
						<CircleIcon class="size-4 text-muted-foreground shrink-0" />
					{/if}
					<span class:text-muted-foreground={!criterion.satisfied}>
						{criterion.description}
					</span>
				</div>
			{/each}
		</div>

		<!-- Hint -->
		{#if challenge.hint}
			{#if showHint}
				<div class="p-3 bg-muted rounded-md">
					<div class="flex items-start gap-2">
						<LightbulbIcon class="size-4 text-yellow-500 shrink-0 mt-0.5" />
						<p class="text-sm">{challenge.hint}</p>
					</div>
				</div>
			{:else}
				<Button variant="ghost" size="sm" onclick={() => (showHint = true)}>
					<LightbulbIcon class="size-4 mr-2" />
					Show Hint
				</Button>
			{/if}
		{/if}

		<!-- Actions -->
		<div class="flex items-center justify-between pt-2">
			<div class="flex items-center gap-1">
				{#if hasPrevious && onPrevious}
					<Button variant="ghost" size="sm" onclick={onPrevious}>
						<ChevronLeftIcon class="size-4 mr-1" />
						Previous
					</Button>
				{/if}
			</div>

			<div class="flex items-center gap-1">
				{#if isComplete}
					{#if hasNext}
						<Button size="sm" onclick={onComplete}>
							Next
							<ChevronRightIcon class="size-4 ml-1" />
						</Button>
					{:else}
						<Button size="sm" onclick={onComplete}>
							Finish
							<CheckCircle2Icon class="size-4 ml-1" />
						</Button>
					{/if}
				{:else if hasNext}
					<Button variant="ghost" size="sm" onclick={onSkip}>
						Skip
						<SkipForwardIcon class="size-4 ml-1" />
					</Button>
				{/if}
			</div>
		</div>
	</CardContent>
</Card>
