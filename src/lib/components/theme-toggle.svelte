<script lang="ts">
    import SunIcon from "@lucide/svelte/icons/sun";
    import MoonIcon from "@lucide/svelte/icons/moon";

    import { resetMode, setMode } from "mode-watcher";
    import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
    import { buttonVariants } from "$lib/components/ui/button/index.js";
    import { isTauri } from "$lib/utils/environment";
    import { m } from "$lib/paraglide/messages.js";

    async function handleSetMode(mode: "light" | "dark" | "system") {
        if (isTauri()) {
            const { setTheme } = await import("@tauri-apps/api/app");
            await setTheme(mode === "system" ? null : mode);
        }
        if (mode === "system") {
            resetMode();
        } else {
            setMode(mode);
        }
    }
</script>

<DropdownMenu.Root>
    <DropdownMenu.Trigger
        class={buttonVariants({ variant: "ghost", size: "icon" })}
    >
        <SunIcon
            class="transition-all! h-[1.2rem] w-[1.2rem] rotate-0 scale-100 dark:-rotate-90 dark:scale-0"
        />
        <MoonIcon
            class="transition-all! absolute h-[1.2rem] w-[1.2rem] rotate-90 scale-0 dark:rotate-0 dark:scale-100"
        />
        <span class="sr-only">{m.theme_toggle()}</span>
    </DropdownMenu.Trigger>
    <DropdownMenu.Content align="end">
        <DropdownMenu.Item onclick={() => handleSetMode("light")}>{m.theme_light()}</DropdownMenu.Item>
        <DropdownMenu.Item onclick={() => handleSetMode("dark")}>{m.theme_dark()}</DropdownMenu.Item>
        <DropdownMenu.Item onclick={() => handleSetMode("system")}>{m.theme_system()}</DropdownMenu.Item>
    </DropdownMenu.Content>
</DropdownMenu.Root>
