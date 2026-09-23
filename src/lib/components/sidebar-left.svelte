<script lang="ts">
	import { page } from "$app/state";
	import { afterNavigate } from "$app/navigation";
	import { resolve } from "$app/paths";
	import * as Sidebar from "$lib/components/ui/sidebar/index.js";
	import { SIDEBAR_WIDTH_ICON } from "$lib/components/ui/sidebar/constants.js";
	import { GraduationCapIcon, Settings2Icon } from "@lucide/svelte";
	import { isTauri } from "$lib/utils/environment";
	import { onboardingStore } from "$lib/stores/onboarding.svelte.js";
	import SidebarManage from "./sidebar-manage.svelte";
	import SidebarLearn from "./sidebar-learn.svelte";

	// Navigation items for the icon sidebar
	const navItems = [
		{ id: "learn", title: "Learn", href: resolve("/(app)/learn"), icon: GraduationCapIcon },
		{ id: "manage", title: "Manage", href: resolve("/(app)/manage"), icon: Settings2Icon },
	] as const;

	// Determine active nav item based on current URL
	// When Learn is disabled, always show manage
	const activeNavItem = $derived(
		onboardingStore.learnEnabled && page.url.pathname.startsWith(resolve("/(app)/learn")) ? "learn" : "manage"
	);

	const sidebar = Sidebar.useSidebar();

	// On mobile the sidebar is a sheet over the page; close it once a link in it
	// has navigated so the chosen lesson or view is visible.
	afterNavigate(() => sidebar.setOpenMobile(false));

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
	class="top-(--header-height) h-[calc(100svh-var(--header-height))] overflow-hidden"
	style={onboardingStore.learnEnabled ? "--sidebar-width: 20rem" : ""}
>
	<!-- Lays the two sidebars out side by side, both in the desktop sidebar and
	     in the mobile sheet (whose content column can't be styled from here).
	     The sheet is portaled outside Sidebar.Provider, so the icon width
	     variable has to be redeclared for it. -->
	<div class="flex h-full w-full min-w-0" style="--sidebar-width-icon: {SIDEBAR_WIDTH_ICON}">
		<!-- Icon Sidebar (first sidebar) - only shown when Learn is enabled -->
		{#if onboardingStore.learnEnabled}
			<Sidebar.Root collapsible="none" class="!w-[calc(var(--sidebar-width-icon)_+_1px)] border-e">
				<Sidebar.Content>
					<Sidebar.Group>
						<Sidebar.GroupContent class="px-0">
							<Sidebar.Menu>
								{#each navItems as item (item.id)}
									<Sidebar.MenuItem>
										<Sidebar.MenuButton
											tooltipContentProps={{
												hidden: false,
											}}
											isActive={activeNavItem === item.id}
											class="px-2"
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

		<!-- Content Sidebar (second sidebar) - show for Manage and Learn. On mobile
		     the whole sidebar lives in a sheet, so it must render there too. -->
		<Sidebar.Root collapsible="none" class={sidebar.isMobile ? "flex flex-1 min-w-0" : "hidden flex-1 min-w-0 md:flex"}>
			{#if activeNavItem === "manage"}
				<SidebarManage {version} />
			{:else if activeNavItem === "learn"}
				<SidebarLearn />
			{/if}
		</Sidebar.Root>
	</div>
</Sidebar.Root>
