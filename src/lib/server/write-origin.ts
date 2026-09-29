/**
 * `X-Seaquel-Origin` (phase 5d, Decision 18): the browser tab a `/api/rpc`
 * call comes from, which Rust puts on the `storageChanged` event of each
 * write so that tab can skip its own change. Not the CORS `Origin`, which
 * `origin.ts` checks.
 *
 * `/api/rpc` forwards it only when it's exactly one value matching
 * {@link WRITE_ORIGIN_PATTERN}; anything else is dropped (the call goes on
 * without an origin), and Rust checks it again. It's never logged. It
 * isn't a security boundary: a lying origin can only hide a change from the
 * user's own tab.
 */

export const WRITE_ORIGIN_HEADER = "x-seaquel-origin";

/** 1–64 of `[A-Za-z0-9_-]`, as Rust's `is_origin`. */
export const WRITE_ORIGIN_PATTERN = /^[A-Za-z0-9_-]{1,64}$/;

/**
 * The request's write origin, or `null` when it has none or it isn't
 * well-formed. Several copies of the header reach `Headers.get` joined by
 * `", "`, which the pattern refuses.
 */
export function writeOrigin(headers: Headers): string | null {
  const value = headers.get(WRITE_ORIGIN_HEADER);
  return value !== null && WRITE_ORIGIN_PATTERN.test(value) ? value : null;
}
