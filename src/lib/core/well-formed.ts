/**
 * Lone surrogates out of what goes to Core. Core's JSON reader
 * (`serde_json`) refuses a lone surrogate, which `JSON.stringify` writes as
 * `\udXXX`, so every request is sent well-formed: `encodeCoreRequest` runs
 * every unary call through `wellFormedJson`, and the stream transports send
 * `wellFormedRequest`. Its own module so `$lib/storage/rust-client` can use
 * it without importing `$lib/core/client`, which imports it.
 */

/**
 * `text` with every lone surrogate replaced by U+FFFD. Core's JSON reader
 * refuses a lone surrogate (which `JSON.stringify` writes as `\udXXX`), and
 * U+FFFD is one UTF-16 unit as the surrogate was, so a cursor offset into
 * the text still points at the same place. `String.prototype.toWellFormed`
 * where the WebView has it.
 */
export function wellFormed(text: string): string {
  const native = (text as { toWellFormed?: () => string }).toWellFormed;
  if (typeof native === "function") return native.call(text);
  let out: string[] | undefined;
  let from = 0;
  for (let i = 0; i < text.length; i++) {
    const c = text.charCodeAt(i);
    if (c < 0xd800 || c > 0xdfff) continue;
    if (c <= 0xdbff) {
      const next = text.charCodeAt(i + 1);
      if (next >= 0xdc00 && next <= 0xdfff) {
        i++;
        continue;
      }
    }
    (out ??= []).push(text.slice(from, i), "\uFFFD");
    from = i + 1;
  }
  return out ? out.join("") + text.slice(from) : text;
}

/**
 * A JSON value with every string in it, keys included, made well-formed
 * (`wellFormed`), for a body the page builds from what the user typed: a
 * window's view state holds the tabs' text and names, and one lone
 * surrogate in them would make Core refuse every save of it (5d-2 Task 7
 * probe). Anything that isn't a string, array or plain object is kept.
 */
export function wellFormedJson<T>(value: T): T {
  if (typeof value === "string") return wellFormed(value) as T;
  if (Array.isArray(value)) return value.map((v: unknown) => wellFormedJson(v)) as T;
  if (
    value !== null &&
    typeof value === "object" &&
    Object.getPrototypeOf(value) === Object.prototype
  ) {
    const out: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(value as Record<string, unknown>)) {
      out[wellFormed(k)] = wellFormedJson(v);
    }
    return out as T;
  }
  return value;
}
