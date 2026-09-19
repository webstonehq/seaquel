<script lang="ts">
    import { page } from "$app/state";
    import { goto } from "$app/navigation";
    import { SidebarInset } from "$lib/components/ui/sidebar";
    import SidebarLeft from "$lib/components/sidebar-left.svelte";
    import { Button } from "$lib/components/ui/button";
    import {
        QueryBuilderWorkspace,
        TablePalette,
        ChallengeCard,
    } from "$lib/components/query-builder";
    import {
        QueryBuilderState,
        setQueryBuilder,
    } from "$lib/hooks/query-builder.svelte";
    import { getLesson, getNextLessonId } from "$lib/tutorial/lessons";
    import { tutorialProgressStore } from "$lib/stores/tutorial-progress.svelte";
    import {
        ResizablePaneGroup,
        ResizablePane,
        ResizableHandle,
    } from "$lib/components/ui/resizable";
    import ArrowLeftIcon from "@lucide/svelte/icons/arrow-left";
    import ArrowRightIcon from "@lucide/svelte/icons/arrow-right";
    import RotateCcwIcon from "@lucide/svelte/icons/rotate-ccw";
    import PlayIcon from "@lucide/svelte/icons/play";
    import Loader2Icon from "@lucide/svelte/icons/loader-2";
    import BookOpenIcon from "@lucide/svelte/icons/book-open";
    import CheckCircle2Icon from "@lucide/svelte/icons/check-circle-2";

    const qb = setQueryBuilder(new QueryBuilderState());

    const lessonId = $derived(page.params.lessonId ?? "");
    const lesson = $derived(lessonId ? getLesson(lessonId) : undefined);

    let currentChallengeIndex = $state(0);
    const completedChallenges = $derived(
        tutorialProgressStore.getCompletedChallenges(lessonId),
    );

    const currentChallenge = $derived(
        lesson?.challenges[currentChallengeIndex],
    );
    const isLessonComplete = $derived(
        lesson ? completedChallenges.size === lesson.challenges.length : false,
    );
    const nextLessonId = $derived(getNextLessonId(lessonId));

    // Track which lesson we've loaded state for
    let loadedForLesson = $state<string | null>(null);

    // Load saved state for the initial challenge when store is initialized
    $effect(() => {
        // Wait for store to be initialized before loading saved state
        // Also reload if the lessonId changes
        if (
            tutorialProgressStore.isInitialized &&
            lesson &&
            lesson.challenges.length > 0 &&
            loadedForLesson !== lessonId
        ) {
            const firstChallenge = lesson.challenges[0];
            const savedState = tutorialProgressStore.getChallengeState(
                lessonId,
                firstChallenge.id,
            );
            if (savedState) {
                qb.fromSerializable(savedState);
            } else {
                qb.reset();
            }
            workspace?.reset?.();
            loadedForLesson = lessonId;
            currentChallengeIndex = 0;
        }
    });

    let workspace = $state<
        ReturnType<typeof QueryBuilderWorkspace> | undefined
    >();
    let isExecuting = $state(false);
    const sqlValue = $derived(qb.customSql ?? qb.generatedSql);
    const canRunQuery = $derived(sqlValue.trim().length > 0 && !isExecuting);

    function handleExecutingChange(value: boolean) {
        isExecuting = value;
    }

    /**
     * Load saved state for a challenge, or reset if none exists
     */
    function loadChallengeState(challengeId: string) {
        const savedState = tutorialProgressStore.getChallengeState(
            lessonId,
            challengeId,
        );
        if (savedState) {
            qb.fromSerializable(savedState);
        } else {
            qb.reset();
        }
        workspace?.reset?.();
    }

    function handleNextChallenge() {
        // Mark current challenge as complete and save its state
        if (currentChallenge && lessonId) {
            const state = qb.toSerializable();
            tutorialProgressStore.completeChallenge(
                lessonId,
                currentChallenge.id,
                state,
            );
        }

        if (lesson && currentChallengeIndex < lesson.challenges.length - 1) {
            // Move to next challenge in the lesson
            const nextIndex = currentChallengeIndex + 1;
            const nextChallenge = lesson.challenges[nextIndex];
            currentChallengeIndex = nextIndex;
            loadChallengeState(nextChallenge.id);
        } else {
            // Last challenge completed - navigate to next lesson
            handleNextLesson();
        }
    }

    function handlePreviousChallenge() {
        if (lesson && currentChallengeIndex > 0) {
            const prevIndex = currentChallengeIndex - 1;
            const prevChallenge = lesson.challenges[prevIndex];
            currentChallengeIndex = prevIndex;
            loadChallengeState(prevChallenge.id);
        }
    }

    function handleGoToChallenge(index: number) {
        if (lesson && index >= 0 && index < lesson.challenges.length) {
            const challenge = lesson.challenges[index];
            currentChallengeIndex = index;
            loadChallengeState(challenge.id);
        }
    }

    function handleSkip() {
        // Skip without marking complete
        if (lesson && currentChallengeIndex < lesson.challenges.length - 1) {
            const nextIndex = currentChallengeIndex + 1;
            const nextChallenge = lesson.challenges[nextIndex];
            currentChallengeIndex = nextIndex;
            loadChallengeState(nextChallenge.id);
        }
    }

    function handleReset() {
        qb.reset();
        workspace?.reset?.();
    }

    function handleRunQuery() {
        workspace?.runQuery?.();
    }

    function handleBack() {
        goto("/learn");
    }

    function handleNextLesson() {
        if (nextLessonId) {
            goto(`/learn/${nextLessonId}`);
        } else {
            goto("/learn");
        }
    }

    function handleRestartLesson() {
        tutorialProgressStore.resetLesson(lessonId);
        qb.reset();
        workspace?.reset?.();
        currentChallengeIndex = 0;
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
                        Challenge {currentChallengeIndex + 1} of {lesson
                            .challenges.length}
                    </p>
                </div>
            </div>
            <div class="flex items-center gap-2">
                <Button variant="outline" size="sm" onclick={handleReset}>
                    <RotateCcwIcon class="size-4 mr-2" />
                    Reset
                </Button>
                <Button
                    size="sm"
                    disabled={!canRunQuery}
                    onclick={handleRunQuery}
                >
                    {#if isExecuting}
                        <Loader2Icon class="size-4 mr-2 animate-spin" />
                        Running...
                    {:else}
                        <PlayIcon class="size-4 mr-2" />
                        Run Query
                    {/if}
                </Button>
            </div>
        </div>

        <!-- Main content -->
        <QueryBuilderWorkspace bind:this={workspace} onExecutingChange={handleExecutingChange}>
            {#snippet leftPanel()}
                <div class="w-64 border-r h-full">
                    <ResizablePaneGroup direction="vertical" class="h-full">
                        <!-- Table palette -->
                        <ResizablePane defaultSize={40} minSize={15}>
                            <div class="h-full overflow-hidden">
                                <TablePalette />
                            </div>
                        </ResizablePane>

                        <ResizableHandle withHandle />

                        <!-- Challenge card -->
                        <ResizablePane defaultSize={60} minSize={20}>
                            <div class="p-3 h-full overflow-y-auto">
                        {#if isLessonComplete && currentChallenge}
                            <div class="space-y-4">
                                <!-- Completion banner -->
                                <div
                                    class="flex items-center gap-2 p-2 bg-green-500/10 rounded-md"
                                >
                                    <CheckCircle2Icon
                                        class="size-5 text-green-500 shrink-0"
                                    />
                                    <span
                                        class="text-sm font-medium text-green-700 dark:text-green-400"
                                        >Lesson Complete!</span
                                    >
                                </div>

                                <!-- Navigation dots -->
                                <div class="flex items-center justify-between">
                                    <div class="flex items-center gap-1.5">
                                        {#each lesson.challenges as ch, i (ch.id)}
                                            <button
                                                type="button"
                                                class={[
                                                    "size-2.5 rounded-full transition-colors",
                                                    i === currentChallengeIndex
                                                        ? "bg-primary ring-2 ring-primary/30"
                                                        : completedChallenges.has(
                                                                ch.id,
                                                            )
                                                          ? "bg-green-500 hover:bg-green-400"
                                                          : "bg-muted-foreground/30 hover:bg-muted-foreground/50",
                                                ]}
                                                onclick={() =>
                                                    handleGoToChallenge(i)}
                                                title={`Challenge ${i + 1}`}
                                            ></button>
                                        {/each}
                                    </div>
                                    <span class="text-xs text-muted-foreground">
                                        {currentChallengeIndex + 1} / {lesson
                                            .challenges.length}
                                    </span>
                                </div>

                                <!-- Challenge title and description -->
                                <div>
                                    <h3 class="font-medium">
                                        {currentChallenge.title}
                                    </h3>
                                    <p
                                        class="text-sm text-muted-foreground mt-1"
                                    >
                                        {currentChallenge.description}
                                    </p>
                                </div>

                                <!-- Criteria checklist (all completed) -->
                                <div class="space-y-2">
                                    {#each currentChallenge.criteria as criterion (criterion.id)}
                                        <div
                                            class="flex items-center gap-2 text-sm"
                                        >
                                            <CheckCircle2Icon
                                                class="size-4 text-green-500 shrink-0"
                                            />
                                            <span>{criterion.description}</span>
                                        </div>
                                    {/each}
                                </div>

                                <!-- Actions -->
                                <div class="flex flex-col gap-2 pt-2">
                                    <Button onclick={handleNextLesson}>
                                        {#if nextLessonId}
                                            Next Lesson
                                            <ArrowRightIcon
                                                class="size-4 ml-2"
                                            />
                                        {:else}
                                            Back to Lessons
                                        {/if}
                                    </Button>
                                    <Button
                                        variant="outline"
                                        size="sm"
                                        onclick={handleRestartLesson}
                                    >
                                        <RotateCcwIcon class="size-4 mr-2" />
                                        Restart Lesson
                                    </Button>
                                </div>
                            </div>
                        {:else if currentChallenge}
                            <ChallengeCard
                                challenge={currentChallenge}
                                challengeIndex={currentChallengeIndex}
                                totalChallenges={lesson.challenges.length}
                                onComplete={handleNextChallenge}
                                onSkip={handleSkip}
                                onPrevious={handlePreviousChallenge}
                                onGoTo={handleGoToChallenge}
                                {completedChallenges}
                                allChallenges={lesson.challenges}
                            />
                        {/if}
                            </div>
                        </ResizablePane>
                    </ResizablePaneGroup>
                </div>
            {/snippet}
        </QueryBuilderWorkspace>
    {:else}
        <!-- Lesson not found -->
        <div class="flex-1 flex items-center justify-center">
            <div class="text-center">
                <BookOpenIcon
                    class="size-12 mx-auto mb-4 text-muted-foreground"
                />
                <h2 class="font-semibold mb-2">Lesson not found</h2>
                <p class="text-sm text-muted-foreground mb-4">
                    The lesson "{lessonId}" doesn't exist.
                </p>
                <Button onclick={handleBack}>Back to Lessons</Button>
            </div>
        </div>
    {/if}
</SidebarInset>
