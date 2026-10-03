import { errorToast } from "$lib/utils/toast";
import { errorCode } from "$lib/core/client";
import { getAi } from "$lib/hooks/database/ai/index";
import { inlineErrorOf } from "$lib/hooks/database/ai/messages";
import { isMac, keySymbols } from "$lib/shortcuts/platform";
import { m } from "$lib/paraglide/messages.js";
import type { QueryEditorContext } from "./types.js";

interface AIPromptError {
  message: string;
  action?: { label: string; fn: () => void };
}

/** The editor's Run shortcut, as the notice names it. */
function runShortcut(): string {
  return isMac()
    ? `${keySymbols.mac.mod}${keySymbols.mac.enter}`
    : `${keySymbols.other.mod}+${keySymbols.other.enter}`;
}

/**
 * The editor's inline prompt (phase 6, Decision 18 and Q9): `ai.generate`
 * with the active saved connection, whose provider, model and sharing
 * Core applies. The SQL is inserted at the cursor and never run: the box
 * says how to run it; the editor's Run is never called from here.
 */
export function createAIInlinePrompt(ctx: QueryEditorContext) {
  const { db } = ctx;

  let open = $state(false);
  let text = $state("");
  let loading = $state(false);
  let error = $state<AIPromptError | null>(null);
  let notice = $state<string | null>(null);

  function handleOpen() {
    text = "";
    error = null;
    notice = null;
    open = true;
  }

  function close() {
    open = false;
    text = "";
    loading = false;
    error = null;
    notice = null;
  }

  const openSettings = () => {
    db.settingsTabs.open("app", "ai-provider");
    close();
  };

  async function submit() {
    if (!text.trim() || loading) return;
    loading = true;
    error = null;
    notice = null;
    const connection = db.state.activeConnection;
    try {
      const sql = await getAi().generate({
        connectionId: connection?.id ?? "",
        providerId: connection?.activeAIProviderId ?? null,
        request: text,
        existingQuery: ctx.getActiveTab()?.query ?? "",
      });
      ctx.getMonacoRef()?.insertText(sql);
      text = "";
      notice = m.ai_inline_inserted({ shortcut: runShortcut() });
    } catch (err) {
      const code = errorCode(err) ?? "UNKNOWN";
      const raw = err instanceof Error ? err.message : String(err);
      const message = raw.startsWith(`${code}: `) ? raw.slice(code.length + 2) : raw;
      const worded = inlineErrorOf(code, message);
      error = {
        message: worded.message,
        ...(worded.action === "configure"
          ? { action: { label: m.ai_inline_configure(), fn: openSettings } }
          : worded.action === "settings"
            ? { action: { label: m.ai_inline_settings(), fn: openSettings } }
            : {}),
      };
      if (worded.toast) errorToast(worded.toast);
    } finally {
      loading = false;
    }
  }

  const focusOnMount = () => (el: HTMLInputElement) => {
    el.focus();
  };

  return {
    get open() {
      return open;
    },
    set open(v: boolean) {
      open = v;
    },
    get text() {
      return text;
    },
    set text(v: string) {
      text = v;
    },
    get loading() {
      return loading;
    },
    get error() {
      return error;
    },
    set error(v: AIPromptError | null) {
      error = v;
    },
    /** After an insert: how to run it (Q9). */
    get notice() {
      return notice;
    },

    handleOpen,
    submit,
    close,
    focusOnMount,
  };
}
