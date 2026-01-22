<script lang="ts">
	import { Button } from "$lib/components/ui/button";
	import { XCircleIcon, CopyIcon } from "@lucide/svelte";
	import { toast } from "svelte-sonner";
	import { errorToast } from "$lib/utils/toast";
	import { m } from "$lib/paraglide/messages.js";

	type Props = {
		statementIndex: number;
		error: string;
		statementSql: string;
	};

	let { statementIndex, error, statementSql }: Props = $props();

	const copyError = async () => {
		try {
			await navigator.clipboard.writeText(error);
			toast.success(m.query_error_copied());
		} catch {
			errorToast(m.query_copy_failed());
		}
	};
</script>

<div class="flex-1 p-4 bg-destructive/10 overflow-auto">
	<div class="flex items-start gap-3">
		<XCircleIcon class="size-5 text-destructive shrink-0 mt-0.5" />
		<div class="flex-1 space-y-3">
			<div class="flex items-start justify-between gap-2">
				<div>
					<h4 class="font-semibold text-destructive">
						{m.query_statement_failed({ n: statementIndex + 1 })}
					</h4>
					<pre class="mt-2 text-sm whitespace-pre-wrap text-destructive/90 font-mono">{error}</pre>
				</div>
				<Button
					variant="ghost"
					size="icon"
					class="shrink-0 size-8 text-destructive/70 hover:text-destructive hover:bg-destructive/10"
					aria-label={m.query_copy_error()}
					onclick={copyError}
				>
					<CopyIcon class="size-4" />
				</Button>
			</div>
			<details class="text-sm">
				<summary class="cursor-pointer text-muted-foreground hover:text-foreground"
					>{m.query_show_sql()}</summary
				>
				<pre class="mt-2 text-xs bg-muted p-3 rounded-md overflow-x-auto font-mono">{statementSql}</pre>
			</details>
		</div>
	</div>
</div>
