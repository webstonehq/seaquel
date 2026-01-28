<!-- src/routes/learn/+page.svelte -->
<script lang="ts">
  import { SidebarInset } from '$lib/components/ui/sidebar';
  import SidebarLeft from '$lib/components/sidebar-left.svelte';
  import { Button } from '$lib/components/ui/button';
  import { Card, CardHeader, CardTitle, CardDescription, CardContent } from '$lib/components/ui/card';
  import { Badge } from '$lib/components/ui/badge';
  import {
    GraduationCapIcon,
    PlayIcon,
    BookOpenIcon,
  } from '@lucide/svelte';
  import { LESSONS, LESSON_SECTIONS } from '$lib/tutorial/lessons';
  import { resolve } from '$app/paths';

  /** Extract first sentence and strip markdown formatting */
  function getDescription(introduction: string): string {
    // Get first line (paragraph)
    const firstLine = introduction.split('\n')[0].trim();
    // Strip markdown bold/italic markers
    return firstLine.replace(/\*\*/g, '').replace(/\*/g, '');
  }
</script>

<SidebarLeft />

<SidebarInset class="flex flex-col h-full overflow-hidden">
  <div class="flex-1 overflow-auto">
    <div class="max-w-4xl mx-auto p-8">
      <!-- Header -->
      <div class="text-center mb-8">
        <GraduationCapIcon class="size-12 mx-auto mb-4 text-primary" />
        <h1 class="text-2xl font-bold mb-2">Learn SQL</h1>
        <p class="text-muted-foreground">
          Master SQL through interactive visual exercises
        </p>
      </div>

      <!-- Sandbox CTA -->
      <Card class="mb-8 border-primary/20 bg-primary/5">
        <CardContent class="flex items-center justify-between p-6">
          <div>
            <h2 class="font-semibold text-lg">SQL Sandbox</h2>
            <p class="text-sm text-muted-foreground">
              Free-form playground to practice building queries
            </p>
          </div>
          <Button href={resolve("/learn/sandbox")}>
            <PlayIcon class="size-4 mr-2" />
            Open Sandbox
          </Button>
        </CardContent>
      </Card>

      <!-- Lessons -->
      <div class="space-y-6">
        {#each LESSON_SECTIONS as section (section.id)}
          <div class="space-y-4">
            <h2 class="font-semibold text-lg">{section.title}</h2>
            {#each section.lessons as lessonId (lessonId)}
              {@const lesson = LESSONS[lessonId]}
              {#if lesson}
                <Card>
                  <CardHeader class="pb-2">
                    <div class="flex items-start justify-between">
                      <div>
                        <CardTitle>{lesson.title}</CardTitle>
                        <CardDescription>{getDescription(lesson.introduction)}</CardDescription>
                      </div>
                      <Badge variant="secondary">{lesson.challenges.length} challenges</Badge>
                    </div>
                  </CardHeader>
                  <CardContent>
                    <Button
                      size="sm"
                      href={resolve("/learn/[lessonId]", { lessonId })}
                    >
                      <BookOpenIcon class="size-4 mr-2" />
                      Start Lesson
                    </Button>
                  </CardContent>
                </Card>
              {/if}
            {/each}
          </div>
        {/each}
      </div>
    </div>
  </div>
</SidebarInset>
