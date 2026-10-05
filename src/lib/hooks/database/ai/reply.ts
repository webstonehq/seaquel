/**
 * Oversized replies.
 *
 * - Core cuts a reply at the most a stored reply may hold (the message
 *   limit, or its own 1 MiB ceiling), ends the turn `tooLong` and stores
 *   the reply ending with `REPLY_CUT_NOTE`. The page takes the note off and
 *   marks the message `cut`, so it shows its own wording live and after a
 *   reload.
 * - A reply over `PLAIN_REPLY_BYTES` is shown as plain text: `marked` can
 *   take seconds on one, or throw (its recursion overflows the stack), and
 *   one it can't render falls back to plain text too.
 */

/** Core's note, as `seaquel_workspace::ai::REPLY_CUT_NOTE` writes it (pinned by `reply.test.ts`). */
export const REPLY_CUT_NOTE =
  "\n\n[Seaquel cut this reply here: it was longer than a stored reply may be.]";

/** Past this many UTF-8 bytes a reply is shown as plain text. */
export const PLAIN_REPLY_BYTES = 64 * 1024;

/** A stored reply's text without Core's cut note, and whether it had one. */
export function splitCutNote(content: string): { content: string; cut: boolean } {
  if (!content.endsWith(REPLY_CUT_NOTE)) return { content, cut: false };
  return { content: content.slice(0, -REPLY_CUT_NOTE.length), cut: true };
}

/** Whether a reply is past `PLAIN_REPLY_BYTES`, so shown as plain text whole. */
export function isPlainReply(text: string): boolean {
  return longerThan(text, PLAIN_REPLY_BYTES);
}

/** Whether `text` is longer than `max` bytes in UTF-8, without encoding a long one. */
function longerThan(text: string, max: number): boolean {
  // Each UTF-16 unit is one to three UTF-8 bytes.
  if (text.length > max) return true;
  if (text.length * 3 <= max) return false;
  return new TextEncoder().encode(text).length > max;
}

/**
 * A reply's text as sanitized HTML, or `null` when it is to be shown as
 * plain text: past `PLAIN_REPLY_BYTES`, or when `parse` throws.
 */
export function replyHtml(
  text: string,
  parse: (text: string) => string,
  sanitize: (html: string) => string,
): string | null {
  if (isPlainReply(text)) return null;
  try {
    return sanitize(parse(text));
  } catch {
    return null;
  }
}
