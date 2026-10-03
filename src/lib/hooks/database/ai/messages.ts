/**
 * How the page words the assistant's errors and tool lines (phase 6,
 * Decision 15). Core sends a code and a message; the page words the code.
 * Only a provider's own message (cut at 1 KiB by Core) appears inside a
 * sentence: Core's other messages are for logs and tests, never shown raw.
 */
import { m } from "$lib/paraglide/messages.js";
import type { AiToolLine } from "$lib/types";

/** Every code a turn, `generate`, `models` or `test` can end with that the page words itself. */
export const AI_ERROR_CODES = [
  "NO_PROVIDER",
  "NO_MODEL",
  "NO_API_KEY",
  "AI_DISABLED",
  "AI_EGRESS_BLOCKED",
  "PROVIDER_ERROR",
  "RATE_LIMITED",
  "TOOL_LIMIT",
  "CHAT_FULL",
  "CONNECTION_MISMATCH",
  "CONNECTION_NOT_FOUND",
  "TURN_IN_PROGRESS",
  "TOO_MANY_REQUESTS",
  "CHAT_NOT_FOUND",
  "AI_PROVIDER_NOT_FOUND",
  "TIMEOUT",
  "NOT_SUPPORTED",
  "VAULT_LOCKED",
  "WS_CLOSED",
  "WORKSPACE_CLOSED",
  "CORE_RESTARTED",
  "AI_PROVIDER_CHANGED",
  "MESSAGE_TOO_LONG",
] as const;

/** A turn's (or a call's) error as the chat shows it. */
export function aiErrorText(code: string, message: string): string {
  switch (code) {
    case "NO_PROVIDER":
      return m.ai_error_no_provider();
    case "NO_MODEL":
      return m.ai_error_no_model();
    case "NO_API_KEY":
      return m.ai_error_no_api_key();
    case "AI_DISABLED":
      return m.ai_error_disabled();
    case "AI_EGRESS_BLOCKED":
      return m.ai_error_egress_blocked();
    case "PROVIDER_ERROR":
      return m.ai_error_provider({ message });
    case "RATE_LIMITED":
      return m.ai_error_rate_limited({ message });
    case "TOOL_LIMIT":
      return m.ai_error_tool_limit();
    case "CHAT_FULL":
      return m.ai_chat_full();
    case "CONNECTION_MISMATCH":
    case "CONNECTION_NOT_FOUND":
    case "WORKSPACE_CLOSED":
    case "CORE_RESTARTED":
      return m.ai_error_connection_gone();
    case "AI_PROVIDER_CHANGED":
      return m.ai_error_provider_changed();
    case "TURN_IN_PROGRESS":
      return m.ai_error_turn_in_progress();
    case "TOO_MANY_REQUESTS":
      return m.ai_error_too_many_requests();
    case "CHAT_NOT_FOUND":
      return m.ai_error_chat_not_found();
    case "AI_PROVIDER_NOT_FOUND":
      return m.ai_error_provider_not_found();
    case "TIMEOUT":
      return m.ai_error_timeout();
    case "NOT_SUPPORTED":
      return m.ai_error_not_supported();
    case "VAULT_LOCKED":
      return m.ai_error_vault_locked();
    case "WS_CLOSED":
      return m.ai_error_ws_closed();
    case "MESSAGE_TOO_LONG":
      return m.ai_error_message_too_long();
    default:
      // Core's message is for logs: it can name an internal limit
      // (`max_message_bytes`, probe F2). The code is enough to report.
      return m.ai_error_generic({ code });
  }
}

/** The inline prompt's error: its text, the action it offers, and an error toast. */
export interface InlineError {
  message: string;
  /** `configure`: add a provider; `settings`: Settings → AI. */
  action?: "configure" | "settings";
  toast?: string;
}

/** How the editor's inline prompt words `ai.generate`'s refusal. */
export function inlineErrorOf(code: string, message: string): InlineError {
  switch (code) {
    case "NO_PROVIDER":
      return { message: m.ai_inline_no_provider(), action: "configure" };
    case "NO_MODEL":
      return { message: m.ai_inline_no_model() };
    case "NO_API_KEY":
      return { message: m.ai_inline_no_api_key(), action: "settings" };
    case "RATE_LIMITED":
      return { message: m.ai_inline_rate_limited() };
    case "PROVIDER_ERROR":
      return { message: m.ai_inline_failed(), toast: message };
    default:
      return { message: m.ai_inline_failed(), toast: aiErrorText(code, message) };
  }
}

/** A tool call's line after its name: its state, rows or error (Q7). */
export function toolLineText(
  line: Pick<AiToolLine, "name" | "state" | "rows" | "truncated" | "code">,
): string {
  switch (line.state) {
    case "running":
      return m.ai_tool_running();
    case "waiting":
      return m.ai_tool_waiting();
    case "ok":
      if (line.rows === undefined) return m.ai_tool_done();
      return line.truncated
        ? m.ai_tool_rows_cut({ count: line.rows })
        : m.ai_tool_rows({ count: line.rows });
    case "error":
      if (line.code === "DENIED") return m.ai_tool_denied();
      if (line.code === "CANCELLED") return m.ai_tool_stopped();
      return line.code ? m.ai_tool_failed_code({ code: line.code }) : m.ai_tool_failed();
  }
}
