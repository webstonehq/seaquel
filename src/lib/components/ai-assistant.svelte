<script lang="ts">
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from "$lib/components/ui/card";
	import { Button } from "$lib/components/ui/button";
	import { Textarea } from "$lib/components/ui/textarea";
	import { ScrollArea } from "$lib/components/ui/scroll-area";
	import { Badge } from "$lib/components/ui/badge";
	import { BotIcon, UserIcon, XIcon, SendIcon, SparklesIcon } from "@lucide/svelte";
	import { fly } from "svelte/transition";

	const db = useDatabase();
	let messageInput = $state("");

	const handleSend = () => {
		if (messageInput.trim()) {
			db.sendAIMessage(messageInput);
			messageInput = "";
		}
	};

	const handleKeydown = (e: KeyboardEvent) => {
		if (e.key === "Enter" && !e.shiftKey) {
			e.preventDefault();
			handleSend();
		}
	};

	const quickPrompts = ["Help me write a query to find all users created in the last 30 days", "How can I optimize this query?", "Where is user email stored?", "Show me how to join orders with users"];
</script>

{#if db.isAIOpen}
	<div class="fixed right-4 bottom-4 w-96 h-[600px] z-50" transition:fly={{ x: 100, duration: 200 }}>
		<Card class="h-full flex flex-col shadow-lg-lg-lg-lg-lg-lg-lg-lg-lg-lg-lg-lg-lg-lg-lg-lg-2xl border-2">
			<CardHeader class="border-b pb-3">
				<div class="flex items-start justify-between">
					<div class="flex items-center gap-2">
						<div class="size-8 rounded-full bg-primary/10 flex items-center justify-center">
							<BotIcon class="size-4 text-primary" />
						</div>
						<div>
							<CardTitle class="text-base">AI Assistant</CardTitle>
							<CardDescription class="text-xs">Ask me anything about your database</CardDescription>
						</div>
					</div>
					<Button size="icon" variant="ghost" class="size-6 [&_svg:not([class*='size-'])]:size-4" onclick={() => db.toggleAI()}>
						<XIcon />
					</Button>
				</div>
			</CardHeader>

			<CardContent class="flex-1 overflow-hidden p-0">
				<ScrollArea class="h-full p-4">
					{#if db.aiMessages.length === 0}
						<div class="space-y-3">
							<div class="text-center py-8">
								<SparklesIcon class="size-12 mx-auto mb-3 text-primary/20" />
								<p class="text-sm text-muted-foreground mb-4">Try asking me something like:</p>
							</div>
							{#each quickPrompts as prompt}
								<Button variant="outline" class="w-full text-left h-auto py-3 px-4 whitespace-normal" onclick={() => (messageInput = prompt)}>
									<span class="text-xs">{prompt}</span>
								</Button>
							{/each}
						</div>
					{:else}
						<div class="flex flex-col gap-4">
							{#each db.aiMessages as message (message.id)}
								<div class={["flex gap-3", message.role === "user" && "flex-row-reverse"]}>
									<div class={["size-8 rounded-full shrink-0 flex items-center justify-center", message.role === "user" ? "bg-primary/10" : "bg-muted"]}>
										{#if message.role === "user"}
											<UserIcon class="size-4 text-primary" />
										{:else}
											<BotIcon class="size-4" />
										{/if}
									</div>
									<div class={["flex-1 space-y-1", message.role === "user" && "text-right"]}>
										<Badge variant={message.role === "user" ? "default" : "secondary"} class="text-xs">
											{message.role === "user" ? "You" : "AI"}
										</Badge>
										<div class={["text-sm rounded-lg p-3", message.role === "user" ? "bg-primary text-primary-foreground ml-8" : "bg-muted mr-8"]}>
											<p class="whitespace-pre-wrap">{message.content}</p>
										</div>
									</div>
								</div>
							{/each}
						</div>
					{/if}
				</ScrollArea>
			</CardContent>

			<CardFooter class="border-t p-3">
				<div class="flex gap-2 w-full">
					<Textarea bind:value={messageInput} placeholder="Ask about your database..." class="min-h-[60px] max-h-[120px] resize-none text-sm" onkeydown={handleKeydown} />
					<Button size="icon" class="shrink-0" onclick={handleSend} disabled={!messageInput.trim()}>
						<SendIcon class="size-4" />
					</Button>
				</div>
			</CardFooter>
		</Card>
	</div>
{/if}
