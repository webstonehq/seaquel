<script lang="ts">
	import { page } from "$app/state";
	import { resolve } from "$app/paths";
	import * as Sidebar from "$lib/components/ui/sidebar/index.js";
	import { GraduationCapIcon, Settings2Icon } from "@lucide/svelte";
	import { isTauri } from "$lib/utils/environment";
	import { onboardingStore } from "$lib/stores/onboarding.svelte.js";
	import SidebarManage from "./sidebar-manage.svelte";
	import SidebarLearn from "./sidebar-learn.svelte";

	// Navigation items for the icon sidebar
	const navItems = [
		{ id: "learn", title: "Learn", href: resolve("/learn"), icon: GraduationCapIcon },
		{ id: "manage", title: "Manage", href: resolve("/manage"), icon: Settings2Icon },
	] as const;

	// Determine active nav item based on current URL
	// When Learn is disabled, always show manage
	const activeNavItem = $derived(
		onboardingStore.learnEnabled && page.url.pathname.startsWith(resolve("/learn")) ? "learn" : "manage"
	);

	let version = $state("");

	$effect(() => {
		if (isTauri()) {
			import("@tauri-apps/api/app").then(({ getVersion }) => {
				getVersion().then((v) => {
					version = v;
				});
			});
		}
	});
</script>

<Sidebar.Root
	collapsible="icon"
	class="top-(--header-height) h-[calc(100svh-var(--header-height))] overflow-hidden [&>[data-sidebar=sidebar]]:flex-row"
	style={onboardingStore.learnEnabled ? "--sidebar-width: 20rem" : ""}
>
	<!-- Icon Sidebar (first sidebar) - only shown when Learn is enabled -->
	{#if onboardingStore.learnEnabled}
		<Sidebar.Root collapsible="none" class="!w-[calc(var(--sidebar-width-icon)_+_1px)] border-e">
			<Sidebar.Content class="pt-2">
				<Sidebar.Group>
					<Sidebar.GroupContent class="px-1.5 md:px-0">
						<Sidebar.Menu>
							{#each navItems as item (item.id)}
								<Sidebar.MenuItem>
									<Sidebar.MenuButton
										tooltipContentProps={{
											hidden: false,
										}}
										isActive={activeNavItem === item.id}
										class="px-2.5 md:px-2"
									>
										{#snippet child({ props })}
											<a href={item.href} {...props}>
												<item.icon />
												<span>{item.title}</span>
											</a>
										{/snippet}
										{#snippet tooltipContent()}
											{item.title}
										{/snippet}
									</Sidebar.MenuButton>
								</Sidebar.MenuItem>
							{/each}
						</Sidebar.Menu>
					</Sidebar.GroupContent>
				</Sidebar.Group>
			</Sidebar.Content>
		</Sidebar.Root>
	{/if}

	<!-- Content Sidebar (second sidebar) - show for Manage and Learn -->
	<Sidebar.Root collapsible="none" class="hidden flex-1 md:flex">
		{#if activeNavItem === "manage"}
			<SidebarManage {version} />
		{:else if activeNavItem === "learn"}
			<SidebarLearn />
		{/if}
	</Sidebar.Root>
</Sidebar.Root>
