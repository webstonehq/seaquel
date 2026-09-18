/**
 * Session + user types for `event.locals`.
 *
 * Shaped to be compatible with Better Auth's session/user but kept narrow so
 * the rest of the app doesn't depend on Better Auth's internal type churn.
 * `hooks.server.ts` populates these from `auth.api.getSession()`.
 */

export interface User {
  id: string;
  email: string;
  name: string | null;
}

export interface Session {
  id: string;
  userId: string;
  expiresAt: number; // unix seconds
}
