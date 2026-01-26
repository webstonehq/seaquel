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
	import SkipForwardIcon from '@lucide/svelte/icons/skip-forward';

	interface Props {
		challenge: Challenge;
		challengeIndex: number;
		totalChallenges: number;
		onComplete?: () => void;
		onSkip?: () => void;
	}

	let { challenge, challengeIndex, totalChallenges, onComplete, onSkip }: Props = $props();

	const qb = useQueryBuilder();

	let showHint = $state(false);

	// Evaluate criteria against current state
	const evaluatedCriteria = $derived<ChallengeCriterion[]>(
		challenge.criteria.map((c) => ({
			...c,
			satisfied: c.check(qb.snapshot, qb.generatedSql)
		}))
	);

	const isComplete = $derived(evaluatedCriteria.every((c) => c.satisfied));
	const completedCount = $derived(evaluatedCriteria.filter((c) => c.satisfied).length);
</script>

<Card class="border-primary/20">
	<CardHeader class="pb-2">
		<div class="flex items-center justify-between">
			<Badge variant="secondary">
				Challenge {challengeIndex + 1} of {totalChallenges}
			</Badge>
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
		<div class="flex justify-between pt-2">
			<Button variant="ghost" size="sm" onclick={onSkip}>
				<SkipForwardIcon class="size-4 mr-2" />
				Skip
			</Button>

			{#if isComplete}
				<Button size="sm" onclick={onComplete}>
					Next Challenge
					<ChevronRightIcon class="size-4 ml-2" />
				</Button>
			{/if}
		</div>
	</CardContent>
</Card>
