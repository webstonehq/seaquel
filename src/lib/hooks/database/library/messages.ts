/**
 * A refused library call, worded for the user. Core's `NAME_TAKEN` names
 * the row that has the name by id (`takenBy`); the page shows that row's
 * name, which it holds. The GUI never compares names itself.
 */
import { m } from "$lib/paraglide/messages.js";
import { errorCode } from "$lib/core/client";
import {
  CONNECTION_NOT_FOUND,
  LABEL_NOT_FOUND,
  LAST_PROJECT,
  NAME_TAKEN,
  PROJECT_NOT_FOUND,
  SAVED_QUERY_NOT_FOUND,
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
      return m.library_name_taken_project({ name });
    }
    case LAST_PROJECT:
      return m.library_last_project();
    case PROJECT_NOT_FOUND:
    case CONNECTION_NOT_FOUND:
    case SAVED_QUERY_NOT_FOUND:
    case LABEL_NOT_FOUND:
      return m.library_not_found();
    default:
      return plainMessage(error, code);
  }
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
