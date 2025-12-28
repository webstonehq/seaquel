import type { AIMessage } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";

/**
 * Manages UI state: AI panel, view switching.
 */
export class UIStateManager {
  constructor(
    private state: DatabaseState,
    private schedulePersistence: (connectionId: string | null) => void
  ) {}

  toggleAI() {
    this.state.isAIOpen = !this.state.isAIOpen;
  }

  sendAIMessage(content: string) {
    const userMessage: AIMessage = {
      id: `msg-${Date.now()}`,
      role: "user",
      content,
      timestamp: new Date(),
    };
    this.state.aiMessages.push(userMessage);

    // Simulate AI response
    setTimeout(() => {
      const responses = [
        "Hey, it's Mike. I appreciate your enthusiasm to play with the AI integration. It's not quite ready yet (boo), but rest assured this is a key feature that is high on my roadmap.",
      ];

      const assistantMessage: AIMessage = {
        id: `msg-${Date.now()}`,
        role: "assistant",
        content: responses[Math.floor(Math.random() * responses.length)],
        timestamp: new Date(),
      };
      this.state.aiMessages.push(assistantMessage);
    }, 1000);
  }

  setActiveView(view: "query" | "schema" | "explain") {
    this.state.activeView = view;
    this.schedulePersistence(this.state.activeConnectionId);
  }
}
