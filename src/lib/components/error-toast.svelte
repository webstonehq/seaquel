<script lang="ts">
    import CopyIcon from "@lucide/svelte/icons/copy";
    import CheckIcon from "@lucide/svelte/icons/check";
    import { writeText } from "@tauri-apps/plugin-clipboard-manager";

    interface Props {
        message: string;
    }

    let { message }: Props = $props();
    let copied = $state(false);

    async function copyMessage(e: MouseEvent) {
        e.stopPropagation();
        await writeText(message);
        copied = true;
        setTimeout(() => (copied = false), 2000);
    }
</script>

<div class="flex">
    <span class="flex-1 text-sm">{message}</span>
    <div class="flex items-center gap-1 shrink-0">
        <button
            type="button"
            class="p-1.5 rounded-md hover:bg-destructive-foreground/10 transition-colors"
            onclick={copyMessage}
            aria-label="Copy error message"
        >
            {#if copied}
                <CheckIcon class="size-4" />
            {:else}
                <CopyIcon class="size-4" />
            {/if}
        </button>
    </div>
</div>
