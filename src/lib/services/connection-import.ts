import type { DatabaseConnection } from "$lib/types";

/** What makes an imported connection the same as a saved one. */
export type ConnectionIdentity = Pick<
  DatabaseConnection,
  "type" | "host" | "port" | "databaseName" | "username"
>;

/**
 * Whether `candidate` is already among `existing`: the same type, host,
 * port, database and user. Two connections on one server (another database
 * or user) are different connections.
 */
export function isAlreadySaved(
  candidate: ConnectionIdentity,
  existing: readonly ConnectionIdentity[],
): boolean {
  return existing.some(
    (c) =>
      c.type === candidate.type &&
      (c.host ?? "") === (candidate.host ?? "") &&
      (c.port ?? 0) === (candidate.port ?? 0) &&
      (c.databaseName ?? "") === (candidate.databaseName ?? "") &&
      (c.username ?? "") === (candidate.username ?? ""),
  );
}
