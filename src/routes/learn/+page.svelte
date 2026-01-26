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
    LockIcon,
  } from '@lucide/svelte';

  const lessons = [
    {
      id: 'select',
      title: 'SELECT Statements',
      description: 'Learn to retrieve data from tables using SELECT queries',
      status: 'available' as const,
      challenges: 5,
    },
    {
      id: 'where',
      title: 'Filtering with WHERE',
      description: 'Filter query results using conditions',
      status: 'locked' as const,
      challenges: 4,
    },
    {
      id: 'joins',
      title: 'JOINs',
      description: 'Combine data from multiple tables',
      status: 'locked' as const,
      challenges: 6,
    },
    {
      id: 'aggregates',
      title: 'Aggregate Functions',
      description: 'Summarize data with COUNT, SUM, AVG, and more',
      status: 'locked' as const,
      challenges: 5,
    },
  ];
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
          <Button href="/learn/sandbox">
            <PlayIcon class="size-4 mr-2" />
            Open Sandbox
          </Button>
        </CardContent>
      </Card>

      <!-- Lessons -->
      <div class="space-y-4">
        <h2 class="font-semibold text-lg">Lessons</h2>
        {#each lessons as lesson (lesson.id)}
          <Card class={lesson.status === 'locked' ? 'opacity-60' : ''}>
            <CardHeader class="pb-2">
              <div class="flex items-start justify-between">
                <div>
                  <CardTitle class="flex items-center gap-2">
                    {lesson.title}
                    {#if lesson.status === 'locked'}
                      <LockIcon class="size-4 text-muted-foreground" />
                    {/if}
                  </CardTitle>
                  <CardDescription>{lesson.description}</CardDescription>
                </div>
                <Badge variant="secondary">{lesson.challenges} challenges</Badge>
              </div>
            </CardHeader>
            <CardContent>
              <Button
                variant={lesson.status === 'available' ? 'default' : 'outline'}
                size="sm"
                disabled={lesson.status === 'locked'}
                href={`/learn/${lesson.id}`}
              >
                {#if lesson.status === 'available'}
                  <BookOpenIcon class="size-4 mr-2" />
                  Start Lesson
                {:else}
                  <LockIcon class="size-4 mr-2" />
                  Locked
                {/if}
              </Button>
            </CardContent>
          </Card>
        {/each}
      </div>
    </div>
  </div>
</SidebarInset>
