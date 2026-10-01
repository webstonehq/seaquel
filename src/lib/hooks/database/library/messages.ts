/**
 * A refused library call, worded for the user. Core's `NAME_TAKEN` names
 * the row that has the name by id (`takenBy`); the page shows that row's
 * name, which it holds. The GUI never compares names itself.
 */
import { m } from "$lib/paraglide/messages.js";
import { errorCode } from "$lib/core/client";
import {
  AI_PROVIDER_NOT_FOUND,
  CHAT_NOT_FOUND,
  CONNECTION_NOT_FOUND,
  DASHBOARD_NOT_FOUND,
  DASHBOARD_VERSION_NOT_FOUND,
  INVALID_ARGUMENT,
  LABEL_NOT_FOUND,
  LAST_PROJECT,
  NAME_TAKEN,
  PROJECT_NOT_FOUND,
  SAVED_QUERY_NOT_FOUND,
  STORAGE_FULL,
  THEME_NOT_FOUND,
  WORKFLOW_NOT_FOUND,
  takenByOf,
} from "./types";

/** A library error with the user's wording; `code` and `takenBy` as Core sent them. */
export class LibraryError extends Error {
  readonly code: string | null;
  readonly takenBy?: string;
  constructor(message: string, code: string | null, takenBy?: string, cause?: unknown) {
    super(message, { cause });
    this.name = "LibraryError";
    this.code = code;
    if (takenBy !== undefined) this.takenBy = takenBy;
  }
}

/** The names the page holds, to name the row a `NAME_TAKEN` points at. */
export type NameOf = (id: string) => string | undefined;

/** Core's message without its `"CODE: "` prefix. */
function plainMessage(error: unknown, code: string | null): string {
  const message = error instanceof Error ? error.message : String(error);
  return code && message.startsWith(`${code}: `) ? message.slice(code.length + 2) : message;
}

/** `error` as the user reads it. */
export function libraryErrorMessage(error: unknown, nameOf: NameOf = () => undefined): string {
  const code = errorCode(error);
  switch (code) {
    case NAME_TAKEN: {
      const id = takenByOf(error);
      const name = id ? nameOf(id) : undefined;
      if (!id || name === undefined) return m.library_name_taken();
      if (id.startsWith("conn-")) return m.library_name_taken_connection({ name });
      if (id.startsWith("saved-")) return m.library_name_taken_saved_query({ name });
      if (id.startsWith("label-")) return m.library_name_taken_label({ name });
      if (id.startsWith("dashboard-")) return m.library_name_taken_dashboard({ name });
      return m.library_name_taken_project({ name });
    }
    case LAST_PROJECT:
      return m.library_last_project();
    case PROJECT_NOT_FOUND:
    case CONNECTION_NOT_FOUND:
    case SAVED_QUERY_NOT_FOUND:
    case LABEL_NOT_FOUND:
    case DASHBOARD_NOT_FOUND:
    case DASHBOARD_VERSION_NOT_FOUND:
    case WORKFLOW_NOT_FOUND:
    case CHAT_NOT_FOUND:
    case THEME_NOT_FOUND:
    case AI_PROVIDER_NOT_FOUND:
      return m.library_not_found();
    case STORAGE_FULL:
      return m.storage_full();
    default:
      return plainMessage(error, code);
  }
}

/**
 * The web limit a refusal names (`INVALID_ARGUMENT` whose message names a
 * `max_*` limit, Decision 27), or `null` for any other error.
 */
export function limitOf(error: unknown): string | null {
  if (errorCode(error) !== INVALID_ARGUMENT) return null;
  const message = error instanceof Error ? error.message : String(error);
  return /\b(max_[a-z_]+)\b/.exec(message)?.[1] ?? null;
}

/**
 * A limit refusal worded by the kind of limit: a name too long, a count
 * reached, or (`null`) a size the caller words for its item.
 */
export function limitMessage(limit: string): string | null {
  if (limit === "max_name_bytes") return m.limit_name_too_long({ limit });
  if (!limit.endsWith("_bytes")) return m.limit_count_reached({ limit });
  return null;
}

/** Whether `error` is Core's refusal of a chat put past the web budget (Q17). */
export function isChatFull(error: unknown): boolean {
  return limitOf(error) === "max_chat_bytes";
}

/** `error` as a `LibraryError` with the user's wording, for a caller to show. */
export function libraryError(error: unknown, nameOf?: NameOf): LibraryError {
  if (error instanceof LibraryError) return error;
  return new LibraryError(
    libraryErrorMessage(error, nameOf),
    errorCode(error),
    takenByOf(error),
    error,
  );
}
