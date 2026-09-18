/**
 * /api/auth/* — hands every request off to Better Auth's request handler.
 *
 * Better Auth exposes a single `handler(request): Response` that handles
 * every verb and sub-path its plugins register (sign-up, sign-in, sign-out,
 * email verification, session endpoints, OAuth callbacks, etc.). Routing is
 * done by Better Auth internally based on the URL path and method.
 */

import { auth } from "$lib/server/auth";
import type { RequestHandler } from "./$types";

const handler: RequestHandler = ({ request }) => auth.handler(request);

export const GET = handler;
export const POST = handler;
export const PUT = handler;
export const PATCH = handler;
export const DELETE = handler;
export const OPTIONS = handler;
