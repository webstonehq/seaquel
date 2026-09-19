<script lang="ts">
    import { m } from "$lib/paraglide/messages.js";
    import { updateStore } from "$lib/stores/update.svelte.js";
    import * as Popover from "$lib/components/ui/popover/index.js";
    import { Button } from "$lib/components/ui/button/index.js";
    import { Separator } from "$lib/components/ui/separator/index.js";
    import ArrowUpRightIcon from "@lucide/svelte/icons/arrow-up-right";
    import PackageIcon from "@lucide/svelte/icons/package";
    import CheckCircleIcon from "@lucide/svelte/icons/circle-check";
    import FileTextIcon from "@lucide/svelte/icons/file-text";
    import MessageCircleIcon from "@lucide/svelte/icons/message-circle";
    import { isTauri } from "$lib/utils/environment";

    const openExternal = (url: string) => {
        if (isTauri()) {
            import("$lib/api/tauri").then(({ openPath }) => {
                openPath(url);
            });
        } else {
            window.open(url, "_blank");
        }
    };

    const formatDate = (dateStr: string | null): string | null => {
        if (!dateStr) return null;
        try {
            const date = new Date(dateStr);
            return date.toLocaleDateString(undefined, {
                year: "numeric",
                month: "short",
                day: "numeric",
            });
        } catch {
            return null;
        }
    };

    const formatSize = (bytes: number | null): string | null => {
        if (!bytes) return null;
        const mb = bytes / (1024 * 1024);
        return `${mb.toFixed(1)} MB`;
    };

    const installLabel = $derived(
        updateStore.isInstalling
            ? "Installing..."
            : updateStore.isDownloaded
              ? "Install and Relaunch"
              : "Downloading...",
    );
</script>

{#if updateStore.upToDate}
    <span class="inline-flex items-center gap-1 rounded-full border border-transparent bg-green-600 text-white text-xs font-medium px-2 py-0.5 mt-0.5">
        <CheckCircleIcon class="size-3" />
        Up to Date
    </span>
{:else if updateStore.visible && updateStore.updateInfo}
    <Popover.Root>
        <Popover.Trigger>
            <button class="inline-flex items-center gap-1 rounded-full border border-transparent bg-blue-600 hover:bg-blue-700 text-white text-xs font-medium px-2 py-0.5 mt-0.5 cursor-pointer transition-colors">
                <PackageIcon class="size-3" />
                Update Available: {updateStore.updateInfo.version}
            </button>
        </Popover.Trigger>
        <Popover.Content class="w-80 p-0" align="end">
            <div class="flex flex-col">
                <div class="p-4 flex flex-col gap-3">
                    <h4 class="font-semibold text-base">Update Available</h4>
                    <div class="grid grid-cols-[auto_1fr] gap-x-2 gap-y-1 text-sm">
                        <span class="text-muted-foreground text-right">Version:</span>
                        <span>{updateStore.updateInfo.version}</span>
                        {#if formatSize(updateStore.updateInfo.size)}
                            <span class="text-muted-foreground text-right">Size:</span>
                            <span>{formatSize(updateStore.updateInfo.size)}</span>
                        {/if}
                        {#if formatDate(updateStore.updateInfo.date)}
                            <span class="text-muted-foreground text-right">Released:</span>
                            <span>{formatDate(updateStore.updateInfo.date)}</span>
                        {/if}
                    </div>

                    <div class="flex items-center gap-2">
                        <Button
                            variant="outline"
                            size="sm"
                            onclick={() => updateStore.skip()}
                        >
                            Skip
                        </Button>
                        <Button
                            variant="outline"
                            size="sm"
                            onclick={() => updateStore.later()}
                        >
                            Later
                        </Button>
                        <div class="flex-1"></div>
                        <Button
                            size="sm"
                            disabled={!updateStore.isDownloaded || updateStore.isInstalling}
                            onclick={() => updateStore.install()}
                        >
                            {installLabel}
                        </Button>
                    </div>
                </div>

                <Separator />

                <button
                    class="flex items-center gap-2 px-4 py-3 text-sm text-muted-foreground hover:text-foreground cursor-pointer w-full transition-colors hover:bg-muted/50"
                    onclick={() => openExternal(`https://seaquel.app/changelog/${updateStore.updateInfo?.version}`)}
                >
                    <FileTextIcon class="size-4" />
                    <span>View Release Notes</span>
                    <div class="flex-1"></div>
                    <ArrowUpRightIcon class="size-4" />
                </button>

                <!-- Opens the web form rather than collecting an email here:
                     consent capture stays in one place, and this popover
                     stays a one-task surface. -->
                <button
                    class="flex items-center gap-2 px-4 py-3 text-sm text-muted-foreground hover:text-foreground cursor-pointer w-full transition-colors hover:bg-muted/50 border-t"
                    onclick={() => openExternal("https://seaquel.app/feedback?from=update")}
                >
                    <MessageCircleIcon class="size-4" />
                    <span>{m.update_help_shape()}</span>
                    <div class="flex-1"></div>
                    <ArrowUpRightIcon class="size-4" />
                </button>
            </div>
        </Popover.Content>
    </Popover.Root>
{/if}
