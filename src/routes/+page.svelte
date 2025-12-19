<script lang="ts">
    import { Toaster } from "$lib/components/ui/sonner";
    import { SidebarInset } from "$lib/components/ui/sidebar";
    import SidebarLeft from "$lib/components/sidebar-left.svelte";
    import SidebarRight from "$lib/components/sidebar-right.svelte";
    import QueryEditor from "$lib/components/query-editor.svelte";
    import TableViewer from "$lib/components/table-viewer.svelte";
    import AIAssistant from "$lib/components/ai-assistant.svelte";
    import { ScrollArea } from "$lib/components/ui/scroll-area";
    import {
        Tabs,
        TabsContent,
        TabsList,
        TabsTrigger,
    } from "$lib/components/ui/tabs";
    import { useDatabase } from "$lib/hooks/database.svelte.js";

    const db = useDatabase();
</script>

<Toaster position="bottom-right" richColors />

<SidebarLeft />
<SidebarInset>
    {#if db.activeConnectionId}
    <Tabs
        value={db.activeView}
        onValueChange={(v) => db.setActiveView(v as typeof db.activeView)}
        class="flex-1 flex flex-col"
    >
        <TabsList
            class="w-full justify-start rounded-none border-b h-10 bg-transparent px-4"
        >
            <TabsTrigger value="query" class="data-[state=active]:bg-background"
                >Query Editor</TabsTrigger
            >
            <TabsTrigger
                value="schema"
                class="data-[state=active]:bg-background"
                >Schema Browser</TabsTrigger
            >
        </TabsList>
        <TabsContent
            value="query"
            class="flex-1 overflow-hidden m-0 data-[state=active]:flex data-[state=active]:flex-col"
        >
            <!-- <ScrollArea> -->
            <QueryEditor />
            <!-- </ScrollArea> -->
        </TabsContent>
        <TabsContent value="schema" class="flex-1 m-0">
            <ScrollArea orientation="both">
                <TableViewer />
            </ScrollArea>
        </TabsContent>
    </Tabs>
    {/if}
</SidebarInset>
<SidebarRight />

<AIAssistant />
