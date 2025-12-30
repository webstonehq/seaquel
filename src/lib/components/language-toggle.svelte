<script lang="ts">
    import GlobeIcon from "@lucide/svelte/icons/globe";
    import CheckIcon from "@lucide/svelte/icons/check";

    import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
    import { buttonVariants } from "$lib/components/ui/button/index.js";
    import { locales, getLocale, setLocale } from "$lib/paraglide/runtime";

    // Strongly typed: TS will error if a new locale is added but not mapped here
    const languages: Record<(typeof locales)[number], { name: string; flag: string }> = {
        en: { name: "English", flag: "🇺🇸" },
        es: { name: "Español", flag: "🇪🇸" },
        ar: { name: "العربية", flag: "🇸🇦" },
        de: { name: "Deutsch", flag: "🇩🇪" },
        ko: { name: "한국어", flag: "🇰🇷" },
        fr: { name: "Français", flag: "🇫🇷" }
    };
</script>

<DropdownMenu.Root>
    <DropdownMenu.Trigger
        class={buttonVariants({ variant: "ghost", size: "icon" })}
    >
        <GlobeIcon class="h-[1.2rem] w-[1.2rem]" />
        <span class="sr-only">Change language</span>
    </DropdownMenu.Trigger>
    <DropdownMenu.Content align="end">
        {#each locales as locale (locale)}
            <DropdownMenu.Item onclick={() => setLocale(locale)}>
                <span class="flex items-center gap-2">
                    <span>{languages[locale].flag}</span>
                    <span>{languages[locale].name}</span>
                    {#if getLocale() === locale}
                        <CheckIcon class="size-4" />
                    {/if}
                </span>
            </DropdownMenu.Item>
        {/each}
    </DropdownMenu.Content>
</DropdownMenu.Root>
