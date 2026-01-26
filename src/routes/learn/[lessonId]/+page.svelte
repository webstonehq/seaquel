<!-- src/routes/learn/[lessonId]/+page.svelte -->
<script lang="ts">
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { SidebarInset } from '$lib/components/ui/sidebar';
	import SidebarLeft from '$lib/components/sidebar-left.svelte';
	import { Button } from '$lib/components/ui/button';
	import * as Resizable from '$lib/components/ui/resizable';
	import {
		QueryBuilderCanvas,
		TablePalette,
		FilterPanel,
		SqlEditor,
		ChallengeCard
	} from '$lib/components/query-builder';
	import { QueryBuilderState, setQueryBuilder } from '$lib/hooks/query-builder.svelte';
	import { getLesson } from '$lib/tutorial/lessons';
	import ArrowLeftIcon from '@lucide/svelte/icons/arrow-left';
	import RotateCcwIcon from '@lucide/svelte/icons/rotate-ccw';
	import BookOpenIcon from '@lucide/svelte/icons/book-open';
	import CheckCircle2Icon from '@lucide/svelte/icons/check-circle-2';

	const qb = setQueryBuilder(new QueryBuilderState());

	const lessonId = $derived(page.params.lessonId ?? '');
	const lesson = $derived(lessonId ? getLesson(lessonId) : undefined);

	let currentChallengeIndex = $state(0);
	let completedChallenges = $state<Set<string>>(new Set());

	const currentChallenge = $derived(lesson?.challenges[currentChallengeIndex]);
	const isLessonComplete = $derived(
		lesson ? completedChallenges.size === lesson.challenges.length : false
	);

	function handleChallengeComplete() {
		if (currentChallenge) {
			completedChallenges.add(currentChallenge.id);
			completedChallenges = completedChallenges; // Trigger reactivity
		}
	}

	function handleNextChallenge() {
		if (lesson && currentChallengeIndex < lesson.challenges.length - 1) {
			currentChallengeIndex++;
			qb.reset();
		}
	}

	function handleSkip() {
		handleNextChallenge();
	}

	function handleReset() {
		qb.reset();
	}

	function handleBack() {
		goto('/learn');
	}
</script>

<SidebarLeft />

<SidebarInset class="flex flex-col h-full overflow-hidden">
	{#if lesson}
		<!-- Header -->
		<div class="flex items-center justify-between px-4 py-2 border-b">
			<div class="flex items-center gap-3">
				<Button variant="ghost" size="icon-sm" onclick={handleBack}>
					<ArrowLeftIcon class="size-4" />
				</Button>
				<div>
					<h1 class="font-semibold">{lesson.title}</h1>
					<p class="text-xs text-muted-foreground">
						Challenge {currentChallengeIndex + 1} of {lesson.challenges.length}
					</p>
				</div>
			</div>
			<div class="flex items-center gap-2">
				<Button variant="outline" size="sm" onclick={handleReset}>
					<RotateCcwIcon class="size-4 mr-2" />
					Reset
				</Button>
			</div>
		</div>

		<!-- Main content -->
		<div class="flex-1 flex min-h-0">
			<!-- Left panel: Challenge + Table palette -->
			<div class="w-64 border-r flex flex-col">
				<!-- Challenge card -->
				<div class="p-3 border-b flex-shrink-0">
					{#if currentChallenge}
						<ChallengeCard
							challenge={currentChallenge}
							challengeIndex={currentChallengeIndex}
							totalChallenges={lesson.challenges.length}
							onComplete={handleNextChallenge}
							onSkip={handleSkip}
						/>
					{:else if isLessonComplete}
						<div class="text-center py-8">
							<CheckCircle2Icon class="size-12 mx-auto mb-4 text-green-500" />
							<h2 class="font-semibold mb-2">Lesson Complete!</h2>
							<p class="text-sm text-muted-foreground mb-4">
								You've completed all challenges.
							</p>
							<Button onclick={handleBack}>Back to Lessons</Button>
						</div>
					{/if}
				</div>

				<!-- Table palette -->
				<div class="flex-1 min-h-0 overflow-hidden">
					<TablePalette />
				</div>
			</div>

			<!-- Canvas + SQL editor -->
			<Resizable.PaneGroup direction="horizontal" class="flex-1">
				<Resizable.Pane defaultSize={60} minSize={30}>
					<div class="flex flex-col h-full">
						<div class="flex-1 min-h-0">
							<QueryBuilderCanvas />
						</div>
						<FilterPanel />
					</div>
				</Resizable.Pane>

				<Resizable.Handle withHandle />

				<Resizable.Pane defaultSize={40} minSize={20}>
					<SqlEditor />
				</Resizable.Pane>
			</Resizable.PaneGroup>
		</div>
	{:else}
		<!-- Lesson not found -->
		<div class="flex-1 flex items-center justify-center">
			<div class="text-center">
				<BookOpenIcon class="size-12 mx-auto mb-4 text-muted-foreground" />
				<h2 class="font-semibold mb-2">Lesson not found</h2>
				<p class="text-sm text-muted-foreground mb-4">
					The lesson "{lessonId}" doesn't exist.
				</p>
				<Button onclick={handleBack}>Back to Lessons</Button>
			</div>
		</div>
	{/if}
</SidebarInset>
