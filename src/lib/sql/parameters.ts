/**
 * The `{{param}}` helpers that don't touch SQL, and the error class
 * `substituteParameters` throws. Moved unchanged from `db/query-params.ts`
 * (deleted in phase 2b); the SQL work is in `seaquel-sql` (`./index.ts`).
 */
import type { QueryParameter, QueryParameterType } from "$lib/types";

/**
 * A parameter value that can't be substituted safely. `substituteParameters`
 * throws it; callers show its message (`errorToast`, or the statement's
 * error result) instead of running the query.
 */
export class ParameterSubstitutionError extends Error {
  override name = "ParameterSubstitutionError";
}

/**
 * Create default parameter definitions from extracted parameter names.
 * All parameters default to 'text' type.
 */
export function createDefaultParameters(paramNames: string[]): QueryParameter[] {
  return paramNames.map((name) => ({
    name,
    type: "text" as const,
    defaultValue: undefined,
    description: undefined,
  }));
}

/**
 * Coerce a string value to the appropriate type based on parameter definition.
 */
export function coerceValue(value: string, type: QueryParameterType): unknown {
  if (value === "" || value === null || value === undefined) {
    return null;
  }

  switch (type) {
    case "number": {
      const num = parseFloat(value);
      return isNaN(num) ? null : num;
    }
    case "boolean":
      return value.toLowerCase() === "true" || value === "1";
    case "date":
    case "datetime":
      // Keep as ISO string for database
      return value;
    case "text":
    default:
      return value;
  }
}
