<script lang="ts">
    import { CopyIcon } from "@lucide/svelte";

    interface Props {
        message: string;
    }

    let { message }: Props = $props();

    function copyMessage() {
        navigator.clipboard.writeText(message);
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
            <CopyIcon class="size-4" />
        </button>
    </div>
</div>
