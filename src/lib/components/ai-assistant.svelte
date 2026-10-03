<script lang="ts">
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { showErrorUnlessShown } from "$lib/errors";
	import { Button } from "$lib/components/ui/button";
	import { Textarea } from "$lib/components/ui/textarea";
	import ChevronRightIcon from "@lucide/svelte/icons/chevron-right";
	import SendIcon from "@lucide/svelte/icons/send";
	import SparklesIcon from "@lucide/svelte/icons/sparkles";
	import PlusIcon from "@lucide/svelte/icons/plus";
	import ChevronDownIcon from "@lucide/svelte/icons/chevron-down";
	import Trash2Icon from "@lucide/svelte/icons/trash-2";
	import ListIcon from "@lucide/svelte/icons/list";
	import SquareIcon from "@lucide/svelte/icons/square";
	import ExternalLinkIcon from "@lucide/svelte/icons/external-link";
	import { marked } from "marked";
	import DOMPurify from "dompurify";
	import { Checkbox } from "$lib/components/ui/checkbox";
	import { Label } from "$lib/components/ui/label";
	import DatabaseIcon from "@lucide/svelte/icons/database";
	import CheckCircleIcon from "@lucide/svelte/icons/check-circle-2";
	import XCircleIcon from "@lucide/svelte/icons/x-circle";
	import { m } from "$lib/paraglide/messages.js";
	import AiModelSwitcher from "$lib/components/ai-model-switcher.svelte";
	import AiMentionPopover from "$lib/components/ai-mention-popover.svelte";
	import { mentionItemsFor, type MentionItem } from "$lib/services/ai-mentions";
	import { aiSettingsStore } from "$lib/stores/ai-settings.svelte";
	import { toolLineText } from "$lib/hooks/database/ai/messages";
	import { isPlainReply, replyHtml } from "$lib/hooks/database/ai/reply";
	import type { AiSegment } from "$lib/types";
	import WrenchIcon from "@lucide/svelte/icons/wrench";
	import * as Sidebar from "$lib/components/ui/sidebar/index.js";
	import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";

	marked.use({ gfm: true, breaks: true });

	const db = useDatabase();
	let messageInput = $state("");
	/** Keyed by approval id: one reply can ask for several approvals in turn. */
	let approvalHandled = $state<Record<string, boolean>>({});
	let scrollRef = $state<HTMLElement | null>(null);
	let userScrolledUp = $state(false);
	let lastScrollTop = 0;
	let textareaRef = $state<HTMLTextAreaElement | null>(null);
	let mentionActive = $state(false);
	let mentionFilter = $state("");
	let mentionStartIndex = $state(0);
	let mentionPopoverRef = $state<ReturnType<typeof AiMentionPopover> | null>(null);

	const schemaSharing = $derived.by(() => {
		const conn = db.state.activeConnection;
		const settings = aiSettingsStore.settings;
		return conn?.aiShareSchema !== undefined ? conn.aiShareSchema : settings.shareSchemaGlobally;
	});

	const mentionItems = $derived(
		mentionItemsFor(
			schemaSharing,
			db.state.activeSchema,
			db.state.projectQueries,
			db.state.projectDashboards,
		),
	);

	function handleScroll() {
		if (!scrollRef) return;
		const { scrollTop, scrollHeight, clientHeight } = scrollRef;
		// User scrolled up — disengage
		if (scrollTop < lastScrollTop) {
			userScrolledUp = true;
		}
		// User scrolled back to bottom — re-engage
		if (userScrolledUp && scrollHeight - scrollTop - clientHeight < 10) {
			userScrolledUp = false;
		}
		lastScrollTop = scrollTop;
	}

	$effect(() => {
		// Subscribe to message content and streaming state to trigger auto-scroll
		const msgs = db.state.aiMessages;
		const lastMsg = msgs.at(-1);
		void lastMsg?.content;
		void lastMsg?.segments;
		void lastMsg?.error;
		void db.state.isAIStreaming;

		if (!scrollRef || userScrolledUp) return;

		requestAnimationFrame(() => {
			if (!scrollRef || userScrolledUp) return;
			scrollRef.scrollTop = scrollRef.scrollHeight;
			lastScrollTop = scrollRef.scrollTop;
		});
	});

	const handleSend = async () => {
		if (!messageInput.trim() || chatFull) return;
		userScrolledUp = false;
		const text = messageInput;
		messageInput = "";
		// Nothing was sent (a chat Core refused to make, say): the text comes back.
		if (!(await db.ui.sendAIMessage(text)) && messageInput === "") messageInput = text;
	};

	const startNewChat = () => {
		if (db.state.isAIStreaming) db.ui.cancelAIStream();
		void db.aiChats.createChat();
	};

	const handleKeydown = (e: KeyboardEvent) => {
		if (mentionActive && mentionPopoverRef) {
			if (mentionPopoverRef.handleKeydown(e)) return;
		}
		if (e.key === "Enter" && !e.shiftKey) {
			e.preventDefault();
			void handleSend();
		}
	};

	function handleInput() {
		if (!textareaRef || mentionItems.length === 0) {
			mentionActive = false;
			return;
		}
		const val = textareaRef.value;
		const cursor = textareaRef.selectionStart;

		// Find the last @ before cursor
		const before = val.slice(0, cursor);
		const atIndex = before.lastIndexOf("@");
		if (atIndex === -1 || (atIndex > 0 && before[atIndex - 1] !== " " && before[atIndex - 1] !== "\n")) {
			mentionActive = false;
			return;
		}

		const fragment = before.slice(atIndex + 1);
		// Close if there's a space and we're not in a quoted mention
		if (fragment.includes(" ") && !fragment.startsWith('"')) {
			mentionActive = false;
			return;
		}

		mentionActive = true;
		mentionFilter = fragment;
		mentionStartIndex = atIndex;
	}

	function selectMention(item: MentionItem) {
		const needsQuotes = item.token.includes(" ");
		const insertText = needsQuotes ? `@"${item.token}" ` : `@${item.token} `;
		const before = messageInput.slice(0, mentionStartIndex);
		const after = messageInput.slice(mentionStartIndex + 1 + mentionFilter.length);
		messageInput = before + insertText + after;
		mentionActive = false;

		requestAnimationFrame(() => {
			if (textareaRef) {
				textareaRef.focus();
				const pos = before.length + insertText.length;
				textareaRef.selectionStart = pos;
				textareaRef.selectionEnd = pos;
			}
		});
	}

	function closeMention() {
		mentionActive = false;
	}

	/** The text as HTML, or `null` to show it as plain text (too long, or `marked` threw). */
	function renderMarkdown(text: string): string | null {
		// Model output can echo database content, so it must be sanitized before {@html}.
		return replyHtml(
			text,
			(t) => marked.parse(t, { async: false }) as string,
			(html) => DOMPurify.sanitize(html),
		);
	}

	type MessageSegment = { type: 'text'; text: string } | { type: 'sql'; code: string };

	function parseMessageContent(content: string): MessageSegment[] {
		const segments: MessageSegment[] = [];
		const regex = /```sql\n([\s\S]*?)```/g;
		let lastIndex = 0;
		let match: RegExpExecArray | null;
		while ((match = regex.exec(content)) !== null) {
			if (match.index > lastIndex) {
				segments.push({ type: 'text', text: content.slice(lastIndex, match.index) });
			}
			segments.push({ type: 'sql', code: match[1].trimEnd() });
			lastIndex = match.index + match[0].length;
		}
		if (lastIndex < content.length) {
			segments.push({ type: 'text', text: content.slice(lastIndex) });
		}
		return segments;
	}

	const quickPrompts = [
		() => m.ai_prompt_users_30_days(),
		() => m.ai_prompt_optimize(),
		() => m.ai_prompt_email(),
		() => m.ai_prompt_join()
	];

	const chats = $derived(db.state.activeConnectionAIChats);
	const activeChatId = $derived(db.state.activeAIChatId);
	/** The web's per-chat budget filled this chat (Q17): sending is off until a new chat. */
	const chatFull = $derived(activeChatId ? !!db.state.aiChatFull[activeChatId] : false);
	const activeChat = $derived(db.state.activeAIChat);
	const userMessages = $derived(db.state.aiMessages.filter((msg) => msg.role === "user"));

	function scrollToMessage(id: string) {
		const el = document.getElementById(`ai-msg-${id}`);
		if (el && scrollRef) {
			userScrolledUp = true;
			el.scrollIntoView({ behavior: "smooth", block: "start" });
		}
	}
</script>

{#snippet textBlock(content: string, role: "user" | "assistant")}
	{#if role === 'assistant' && isPlainReply(content)}
		<!-- Past 64 KiB a reply is plain text: no Markdown, no SQL blocks (probe F2). -->
		<p class="whitespace-pre-wrap break-words select-text text-sm text-foreground">{content}</p>
	{:else}
	{#each parseMessageContent(content) as segment, si (si)}
		{#if segment.type === 'text'}
			{#if segment.text.trim()}
				{#if role === 'assistant'}
					{@const html = renderMarkdown(segment.text)}
					{#if html === null}
						<p class="whitespace-pre-wrap break-words select-text text-sm text-foreground">{segment.text}</p>
					{:else}
						<div class="prose prose-sm dark:prose-invert max-w-none select-text prose-p:my-1 prose-headings:mt-2 prose-headings:mb-1 prose-ul:my-1 prose-ol:my-1 prose-li:my-0 prose-pre:my-1 prose-code:before:content-none prose-code:after:content-none text-sm">
							{@html html}
						</div>
					{/if}
				{:else}
					<p class="whitespace-pre-wrap select-text text-sm text-foreground">{segment.text}</p>
				{/if}
			{/if}
		{:else}
			<div class="mt-1 rounded border bg-background overflow-hidden">
				<div class="flex items-center justify-between px-2 py-1 border-b">
					<span class="text-xs text-muted-foreground font-mono">SQL</span>
					<Button size="sm" variant="ghost" class="h-6 text-xs gap-1 px-2" onclick={() => { const tabId = db.queryTabs.add(m.ai_sql_tab_title(), segment.code.trim()); if (tabId) db.ui.setActiveView("query"); }}>
						<ExternalLinkIcon class="size-3" aria-hidden="true" />
						{m.ai_open_in_editor()}
					</Button>
				</div>
				<pre class="text-xs font-mono p-2 whitespace-pre-wrap select-text overflow-x-auto">{segment.code}</pre>
			</div>
		{/if}
	{/each}
	{/if}
{/snippet}

{#snippet toolLine(line: Extract<AiSegment, { type: "tool" }>)}
	<!-- Q7: one line per tool call: the tool, its SQL, then its rows or its error. -->
	<div class="my-1 flex items-center gap-1.5 rounded border bg-muted/40 px-2 py-1 text-xs" data-tool-call={line.callId}>
		<WrenchIcon class="size-3 shrink-0 text-muted-foreground" aria-hidden="true" />
		<span class="font-mono shrink-0">{line.name}</span>
		{#if line.sql}
			<span class="font-mono truncate text-muted-foreground min-w-0" title={line.sql}>{line.sql}</span>
		{/if}
		<span class="ml-auto shrink-0 {line.state === 'error' ? 'text-destructive' : 'text-muted-foreground'}">{toolLineText(line)}</span>
	</div>
{/snippet}

<Sidebar.Header class="border-b px-4 py-3">
	<div class="flex items-start justify-between">
		<div class="flex items-center gap-2">
			<div class="size-8 rounded-full bg-primary/10 flex items-center justify-center">
				<SparklesIcon class="size-4 text-primary" />
			</div>
			<div>
				<p class="text-sm font-semibold">{m.ai_title()}</p>
				<p class="text-xs text-muted-foreground">{m.ai_description()}</p>
			</div>
		</div>
		<div class="flex items-center gap-0.5">
			<Button size="icon" variant="ghost" class="size-6 [&_svg:not([class*='size-'])]:size-4" aria-label={m.ai_new_chat()} onclick={startNewChat}>
				<PlusIcon />
			</Button>
			<Button size="icon" variant="ghost" class="size-6 [&_svg:not([class*='size-'])]:size-4" aria-label={m.ai_close()} onclick={() => db.ui.toggleAI()}>
				<ChevronRightIcon />
			</Button>
		</div>
	</div>
	{#if chats.length > 0}
		<div class="mt-2 flex items-center gap-1">
			<DropdownMenu.Root>
				<DropdownMenu.Trigger>
					<Button variant="outline" size="sm" class="flex-1 justify-between h-7 text-xs">
						<span class="truncate">{activeChat?.title ?? m.ai_new_chat()}</span>
						<ChevronDownIcon class="size-3 shrink-0 opacity-50" />
					</Button>
				</DropdownMenu.Trigger>
				<DropdownMenu.Portal>
					<DropdownMenu.Content class="w-[var(--bits-dropdown-menu-trigger-width)] max-h-64 overflow-y-auto" align="start">
						<DropdownMenu.RadioGroup value={activeChatId ?? undefined} onValueChange={(id) => { if (id) { if (db.state.isAIStreaming) db.ui.cancelAIStream(); db.aiChats.switchChat(id); } }}>
							{#each chats as chat (chat.id)}
								<DropdownMenu.RadioItem value={chat.id} class="text-xs group pr-1">
									<span class="truncate flex-1">{chat.title}</span>
									<button
										class="opacity-0 group-hover:opacity-100 shrink-0 p-0.5 rounded hover:bg-destructive/10 hover:text-destructive transition-opacity"
										onclick={(e) => { e.stopPropagation(); db.aiChats.deleteChat(chat.id); }}
										aria-label={m.ai_delete_chat()}
									>
										<Trash2Icon class="size-3" />
									</button>
								</DropdownMenu.RadioItem>
							{/each}
						</DropdownMenu.RadioGroup>
					</DropdownMenu.Content>
				</DropdownMenu.Portal>
			</DropdownMenu.Root>
		</div>
	{/if}
	{#if userMessages.length > 1}
		<div class="mt-1 flex items-center gap-1">
			<DropdownMenu.Root>
				<DropdownMenu.Trigger>
					<Button variant="ghost" size="sm" class="flex-1 justify-between h-7 text-xs text-muted-foreground">
						<span class="flex items-center gap-1.5">
							<ListIcon class="size-3 shrink-0" />
							{m.ai_jump_to_message()}
						</span>
						<ChevronDownIcon class="size-3 shrink-0 opacity-50" />
					</Button>
				</DropdownMenu.Trigger>
				<DropdownMenu.Content class="w-[var(--bits-dropdown-menu-trigger-width)] max-h-64 overflow-y-auto" align="start">
					{#each userMessages as msg (msg.id)}
						<DropdownMenu.Item class="text-xs" onSelect={() => scrollToMessage(msg.id)}>
							<span class="truncate">{msg.content.length > 60 ? msg.content.slice(0, 60) + "…" : msg.content}</span>
						</DropdownMenu.Item>
					{/each}
				</DropdownMenu.Content>
			</DropdownMenu.Root>
		</div>
	{/if}
</Sidebar.Header>

<div class="relative flex-1 min-h-0 flex flex-col">
<Sidebar.Content bind:ref={scrollRef} onscroll={handleScroll}>
	<div class="p-4">
		{#if db.state.aiMessages.length === 0}
			<div class="space-y-3">
				<div class="text-center py-8">
					<SparklesIcon class="size-12 mx-auto mb-3 text-primary/20" />
					<p class="text-sm text-muted-foreground mb-4">{m.ai_try_asking()}</p>
				</div>
				{#each quickPrompts as prompt, i (i)}
					<Button variant="outline" class="w-full text-start h-auto py-3 px-4 whitespace-normal" onclick={() => (messageInput = prompt())}>
						<span class="text-xs">{prompt()}</span>
					</Button>
				{/each}
			</div>
		{:else}
			<div class="flex flex-col gap-5">
				{#each db.state.aiMessages as message (message.id)}
					<div id="ai-msg-{message.id}" class={message.role === "user" ? "border-l-2 border-primary/40 pl-3" : ""}>
								{#if message.pendingModelSelection}
									<p class="text-sm text-muted-foreground mb-3">{m.ai_choose_model_to_send()}</p>
									<AiModelSwitcher
										providerId={db.state.activeConnection?.activeAIProviderId ?? null}
										model={db.state.activeConnection?.activeAIModel ?? null}
										onSelect={async (pid, mod) => {
											const conn = db.state.activeConnection;
											if (!conn) return;
											try {
												await db.setConnectionAIModel(conn.id, pid, mod);
											} catch (error) {
												showErrorUnlessShown(error);
												return;
											}
											db.ui.retryPendingMessage(message.id);
										}}
									/>
								{:else}
								{#if message.segments?.some((s) => s.type === "tool")}
									{#each message.segments as part, pi (pi)}
										{#if part.type === "text"}
											{@render textBlock(part.text, message.role)}
										{:else}
											{@render toolLine(part)}
										{/if}
									{/each}
								{:else if message.content}
									{@render textBlock(message.content, message.role)}
								{/if}
								{#if message.truncated}
									<p class="mt-1 text-xs text-muted-foreground">{m.ai_reply_truncated()}</p>
								{/if}
								{#if message.cut}
									<p class="mt-1 text-xs text-muted-foreground">{m.ai_reply_too_long()}</p>
								{/if}
								{#if message.error}
									<!-- Plain text: no Markdown in an error (review M1). -->
									<p class="mt-1 rounded border border-destructive/40 bg-destructive/5 px-2 py-1.5 text-xs text-destructive whitespace-pre-wrap select-text" role="alert">{message.error}</p>
								{/if}
								{/if}
								{#if message.pendingApproval}
									{@const approval = message.pendingApproval}
									<div class="mt-2 space-y-3">
										<div class="rounded border bg-background p-2 space-y-1">
											<p class="text-xs font-medium text-muted-foreground">{m.ai_approval_query()}</p>
											<pre class="text-xs font-mono whitespace-pre-wrap break-all">{approval.query}</pre>
										</div>
										<div class="flex items-center gap-1.5 text-xs text-muted-foreground">
											<DatabaseIcon class="size-3" />
											<span>{approval.connectionName}</span>
										</div>
										<div class="flex items-center gap-1.5">
											<Checkbox
												id="allow-all-{approval.id}"
												checked={approval.allowAllTicked}
												onCheckedChange={(v) => approval.setAllowAllTicked(v === true)}
											/>
											<Label for="allow-all-{approval.id}" class="text-xs font-normal cursor-pointer">
												{m.ai_allow_all_connection()}
											</Label>
										</div>
										{#if approval.connectionType === "mssql"}
											<p class="text-xs text-muted-foreground">{m.ai_allow_all_hint_mssql()}</p>
										{:else if approval.connectionType === "duckdb"}
											<p class="text-xs text-muted-foreground">{m.ai_allow_all_hint_duckdb()}</p>
										{/if}
										<div class="flex gap-2">
											<Button
												size="sm"
												class="flex-1 gap-1.5"
												disabled={approvalHandled[approval.id] ?? false}
												onclick={() => {
													if (approvalHandled[approval.id]) return;
													approvalHandled = { ...approvalHandled, [approval.id]: true };
													approval.approve();
												}}
											>
												<CheckCircleIcon class="size-3.5" aria-hidden="true" />
												{m.ai_approval_allow()}
											</Button>
											<Button
												size="sm"
												variant="outline"
												class="flex-1 gap-1.5"
												disabled={approvalHandled[approval.id] ?? false}
												onclick={() => {
													if (approvalHandled[approval.id]) return;
													approvalHandled = { ...approvalHandled, [approval.id]: true };
													approval.deny();
												}}
											>
												<XCircleIcon class="size-3.5" aria-hidden="true" />
												{m.ai_approval_deny()}
											</Button>
										</div>
									</div>
								{/if}
					</div>
				{/each}
				{#if db.state.isAIStreaming && !db.state.aiMessages.at(-1)?.content}
					<div class="flex items-center gap-1 py-2">
						<span class="size-1.5 rounded-full bg-muted-foreground/60 animate-bounce [animation-delay:0ms]"></span>
						<span class="size-1.5 rounded-full bg-muted-foreground/60 animate-bounce [animation-delay:150ms]"></span>
						<span class="size-1.5 rounded-full bg-muted-foreground/60 animate-bounce [animation-delay:300ms]"></span>
					</div>
				{/if}
			</div>
		{/if}
	</div>
</Sidebar.Content>
{#if userScrolledUp && db.state.aiMessages.length > 0}
	<div class="absolute bottom-2 left-1/2 -translate-x-1/2 z-10">
		<Button
			size="sm"
			variant="secondary"
			class="rounded-full shadow-md h-7 px-3 gap-1 text-xs opacity-90 hover:opacity-100 transition-opacity"
			onclick={() => {
				if (!scrollRef) return;
				userScrolledUp = false;
				scrollRef.scrollTo({ top: scrollRef.scrollHeight, behavior: "smooth" });
			}}
		>
			<ChevronDownIcon class="size-3" />
			{m.ai_new_messages()}
		</Button>
	</div>
{/if}
</div>

<Sidebar.Footer class="border-t p-3">
	<div class="flex flex-col gap-2 w-full">
		{#if chatFull}
			<div
				class="flex items-center justify-between gap-3 rounded-md border border-amber-500/40 bg-amber-500/10 px-3 py-2 text-xs"
				role="status"
			>
				<span>{m.ai_chat_full()}</span>
				<Button size="sm" variant="outline" class="h-7 shrink-0" onclick={startNewChat}>
					{m.ai_new_chat()}
				</Button>
			</div>
		{/if}
		<div class="flex gap-2">
			<div class="relative flex-1">
				{#if mentionActive}
					<AiMentionPopover
						bind:this={mentionPopoverRef}
						items={mentionItems}
						filter={mentionFilter}
						onSelect={selectMention}
						onClose={closeMention}
					/>
				{/if}
				<Textarea
					bind:ref={textareaRef}
					bind:value={messageInput}
					placeholder={m.ai_placeholder()}
					class="min-h-[60px] max-h-[120px] resize-none text-sm"
					disabled={chatFull}
					onkeydown={handleKeydown}
					oninput={handleInput}
				/>
			</div>
			{#if db.state.isAIStreaming}
				<Button size="icon" variant="destructive" class="shrink-0" aria-label={m.ai_stop()} onclick={() => db.ui.cancelAIStream()}>
					<SquareIcon class="size-4" />
				</Button>
			{:else}
				<Button size="icon" class="shrink-0" aria-label={m.ai_send()} onclick={handleSend} disabled={!messageInput.trim() || chatFull}>
					<SendIcon class="size-4" />
				</Button>
			{/if}
		</div>
		<div class="flex items-center gap-2">
			<span class="text-xs text-muted-foreground">{m.settings_ai_model_switcher_label()}:</span>
			<AiModelSwitcher
				providerId={db.state.activeConnection?.activeAIProviderId ?? null}
				model={db.state.activeConnection?.activeAIModel ?? null}
				onSelect={async (pid, mod) => {
					const conn = db.state.activeConnection;
					if (!conn) return;
					await db.setConnectionAIModel(conn.id, pid, mod).catch(showErrorUnlessShown);
				}}
			/>
		</div>
	</div>
</Sidebar.Footer>
