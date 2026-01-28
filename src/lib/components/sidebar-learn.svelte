<script lang="ts">
	import { SvelteSet } from "svelte/reactivity";
	import { page } from "$app/state";
	import * as Sidebar from "$lib/components/ui/sidebar/index.js";
	import { Badge } from "$lib/components/ui/badge";
	import { ChevronRightIcon, BookOpenIcon, BoxIcon } from "@lucide/svelte";
	import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "$lib/components/ui/collapsible";
	import { tutorialProgressStore } from "$lib/stores/tutorial-progress.svelte.js";
	import { LESSONS, LESSON_SECTIONS } from "$lib/tutorial/lessons";
	import { m } from "$lib/paraglide/messages.js";

	let expandedTutorialSections = new SvelteSet<string>(["basics"]);

	// Auto-expand section containing the active lesson
	$effect(() => {
		const match = page.url.pathname.match(/^\/learn\/([^/]+)$/);
		if (match) {
			const lessonId = match[1];
			const section = LESSON_SECTIONS.find(s => s.lessons.includes(lessonId));
			if (section) {
				expandedTutorialSections.add(section.id);
			}
		}
	});
</script>

<!-- Header -->
<Sidebar.Header class="p-0 py-1">
	<Sidebar.Group class="py-2">
		<div class="flex items-center gap-2 px-3 py-1">
			<BookOpenIcon class="size-4 text-sidebar-foreground/70" />
			<span class="text-xs font-medium text-sidebar-foreground/70">{m.learn_sql_tutorial()}</span>
		</div>
	</Sidebar.Group>
</Sidebar.Header>

<!-- Content -->
<Sidebar.Content>
	<!-- Sandbox Link -->
	<Sidebar.Group class="pb-0">
		<Sidebar.GroupContent>
			<Sidebar.Menu>
				<Sidebar.MenuItem>
					<Sidebar.MenuButton isActive={page.url.pathname === '/learn/sandbox'}>
						{#snippet child({ props })}
							<a href="/learn/sandbox" {...props}>
								<BoxIcon class="size-4" />
								<span>{m.learn_sandbox()}</span>
							</a>
						{/snippet}
					</Sidebar.MenuButton>
				</Sidebar.MenuItem>
			</Sidebar.Menu>
		</Sidebar.GroupContent>
	</Sidebar.Group>

	<!-- Lessons -->
	<Sidebar.Group>
		<Sidebar.GroupContent>
			<Sidebar.Menu>
				{#each LESSON_SECTIONS as section (section.id)}
					<Collapsible open={expandedTutorialSections.has(section.id)} onOpenChange={() => {
						if (expandedTutorialSections.has(section.id)) {
							expandedTutorialSections.delete(section.id);
						} else {
							expandedTutorialSections.add(section.id);
						}
					}}>
						<Sidebar.MenuItem>
							<CollapsibleTrigger>
								{#snippet child({ props })}
									<Sidebar.MenuButton {...props}>
										<ChevronRightIcon class={["size-4 transition-transform", expandedTutorialSections.has(section.id) && "rotate-90"]} />
										<span class="flex-1 font-medium">{section.title}</span>
										<Badge variant="secondary" class="text-xs min-w-6 justify-center">{section.lessons.length}</Badge>
									</Sidebar.MenuButton>
								{/snippet}
							</CollapsibleTrigger>
							<CollapsibleContent class="flex">
								<Sidebar.Menu class="ms-4 border-s border-sidebar-border ps-2">
									{#each section.lessons as lessonId (lessonId)}
										{@const lesson = LESSONS[lessonId]}
										{@const totalChallenges = lesson?.challenges.length ?? 0}
										{@const completedCount = tutorialProgressStore.getCompletedCount(lessonId)}
										{@const isFullyCompleted = totalChallenges > 0 && completedCount >= totalChallenges}
										{@const progressPercent = totalChallenges > 0 ? (completedCount / totalChallenges) * 100 : 0}
										{#if lesson}
											<Sidebar.MenuItem>
												<Sidebar.MenuButton
													isActive={page.url.pathname === `/learn/${lessonId}`}
													class="flex items-center gap-2"
												>
													{#snippet child({ props })}
														<a href={`/learn/${lessonId}`} {...props}>
															<!-- Pie chart progress indicator -->
															<svg class="size-4 -rotate-90" viewBox="0 0 16 16">
																<!-- Background circle -->
																<circle
																	cx="8"
																	cy="8"
																	r="6"
																	fill="none"
																	stroke="currentColor"
																	stroke-width="2"
																	class="text-muted-foreground/30"
																/>
																<!-- Progress arc -->
																{#if progressPercent > 0}
																	<circle
																		cx="8"
																		cy="8"
																		r="6"
																		fill="none"
																		stroke="currentColor"
																		stroke-width="2"
																		stroke-linecap="round"
																		stroke-dasharray={2 * Math.PI * 6}
																		stroke-dashoffset={2 * Math.PI * 6 * (1 - progressPercent / 100)}
																		class={isFullyCompleted ? "text-green-500" : "text-primary"}
																	/>
																{/if}
																<!-- Checkmark for completed -->
																{#if isFullyCompleted}
																	<path
																		d="M5 8.5L7 10.5L11 6"
																		fill="none"
																		stroke="currentColor"
																		stroke-width="1.5"
																		stroke-linecap="round"
																		stroke-linejoin="round"
																		class="text-green-500"
																		transform="rotate(90 8 8)"
																	/>
																{/if}
															</svg>
															<span class="flex-1 text-sm">{lesson.title}</span>
															{#if totalChallenges > 0}
																<span class="text-xs text-muted-foreground tabular-nums">{completedCount}/{totalChallenges}</span>
															{/if}
														</a>
													{/snippet}
												</Sidebar.MenuButton>
											</Sidebar.MenuItem>
										{/if}
									{/each}
								</Sidebar.Menu>
							</CollapsibleContent>
						</Sidebar.MenuItem>
					</Collapsible>
				{/each}
			</Sidebar.Menu>
		</Sidebar.GroupContent>
	</Sidebar.Group>
</Sidebar.Content>

<!-- Footer -->
<Sidebar.Footer class="p-4">
	<div class="text-xs text-muted-foreground flex justify-between">
		<span>
			{m.learn_lessons_count({ count: LESSON_SECTIONS.reduce((acc, s) => acc + s.lessons.length, 0) })}
		</span>
	</div>
</Sidebar.Footer>
