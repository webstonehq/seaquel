/**
 * /health — simple liveness probe for the tenant container's Node process.
 *
 * Returns 200 "ok" as long as Node is accepting requests. Does NOT verify
 * the loopback Rust service — that's `/api/db/*`'s responsibility. Keeping
 * the two decoupled means a Rust-only hiccup doesn't trip the whole
 * container's liveness probe.
 */

export function GET(): Response {
  return new Response("ok", {
    status: 200,
    headers: { "content-type": "text/plain; charset=utf-8" },
  });
}
