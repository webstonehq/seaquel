<script lang="ts">
	import { buttonVariants } from "$lib/components/ui/button";
	import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
	import { DownloadIcon, CopyIcon, ChevronDownIcon } from "@lucide/svelte";
	import { m } from "$lib/paraglide/messages.js";
	import type { ExportFormat } from "$lib/utils/export-formats.js";

	type Props = {
		onExport: (format: ExportFormat) => void;
		onCopy: (format: ExportFormat) => void;
	};

	let { onExport, onCopy }: Props = $props();
</script>

<DropdownMenu.Root>
	<DropdownMenu.Trigger
		class={buttonVariants({
			variant: "ghost",
			size: "sm"
		}) + " h-7 gap-1.5 px-2"}
	>
		<DownloadIcon class="size-3.5" />
		{m.query_export()}
		<ChevronDownIcon class="size-3" />
	</DropdownMenu.Trigger>
		<DropdownMenu.Content align="end" class="w-48">
			<DropdownMenu.Group>
				<DropdownMenu.GroupHeading>{m.query_download()}</DropdownMenu.GroupHeading>
				<DropdownMenu.Item onclick={() => onExport("csv")}>
					<DownloadIcon class="size-4 me-2" />
					CSV
				</DropdownMenu.Item>
				<DropdownMenu.Item onclick={() => onExport("json")}>
					<DownloadIcon class="size-4 me-2" />
					JSON
				</DropdownMenu.Item>
				<DropdownMenu.Item onclick={() => onExport("sql")}>
					<DownloadIcon class="size-4 me-2" />
					SQL INSERT
				</DropdownMenu.Item>
				<DropdownMenu.Item onclick={() => onExport("markdown")}>
					<DownloadIcon class="size-4 me-2" />
					Markdown
				</DropdownMenu.Item>
			</DropdownMenu.Group>
			<DropdownMenu.Separator />
			<DropdownMenu.Group>
				<DropdownMenu.GroupHeading>{m.query_copy_to_clipboard()}</DropdownMenu.GroupHeading>
				<DropdownMenu.Item onclick={() => onCopy("csv")}>
					<CopyIcon class="size-4 me-2" />
					CSV
				</DropdownMenu.Item>
				<DropdownMenu.Item onclick={() => onCopy("json")}>
					<CopyIcon class="size-4 me-2" />
					JSON
				</DropdownMenu.Item>
				<DropdownMenu.Item onclick={() => onCopy("sql")}>
					<CopyIcon class="size-4 me-2" />
					SQL INSERT
				</DropdownMenu.Item>
				<DropdownMenu.Item onclick={() => onCopy("markdown")}>
					<CopyIcon class="size-4 me-2" />
					Markdown
				</DropdownMenu.Item>
			</DropdownMenu.Group>
		</DropdownMenu.Content>
</DropdownMenu.Root>
