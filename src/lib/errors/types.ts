/**
 * Centralized error handling types for Seaquel.
 * Provides consistent error representation across the application.
 */

/**
 * Error codes for categorizing application errors.
 * Used to determine severity and handling behavior.
 */
export type ErrorCode =
	| 'CONNECTION_FAILED'
	| 'QUERY_FAILED'
	| 'PERSISTENCE_FAILED'
	| 'SSH_TUNNEL_FAILED'
	| 'SCHEMA_LOAD_FAILED'
	| 'VALIDATION_FAILED'
	| 'EXPORT_FAILED'
	| 'IMPORT_FAILED'
	| 'UNKNOWN';

/**
 * Structured application error with user-friendly messaging.
 */
export interface AppError {
	/** Error category for routing and severity determination */
	code: ErrorCode;
	/** Technical error message for debugging */
	message: string;
	/** User-friendly message suitable for display */
	userMessage: string;
	/** Additional context for debugging */
	context?: Record<string, unknown>;
	/** Original error if wrapping another error */
	cause?: Error;
}

/**
 * Result type for operations that can fail.
 * Encourages explicit error handling over try/catch.
 *
 * @example
 * const result = await connectToDatabase(config);
 * if (result.ok) {
 *   console.log('Connected:', result.value);
 * } else {
 *   handleError(result.error);
 * }
 */
export type Result<T, E = AppError> = { ok: true; value: T } | { ok: false; error: E };

/**
 * Creates a structured AppError with consistent formatting.
 */
export function createError(
	code: ErrorCode,
	message: string,
	userMessage?: string,
	context?: Record<string, unknown>,
	cause?: Error
): AppError {
	return {
		code,
		message,
		userMessage: userMessage ?? message,
		context,
		cause
	};
}

/**
 * Creates a successful Result.
 */
export function ok<T>(value: T): Result<T, never> {
	return { ok: true, value };
}

/**
 * Creates a failed Result.
 */
export function err<E = AppError>(error: E): Result<never, E> {
	return { ok: false, error };
}

/**
 * Extracts a human-readable error message from various error formats.
 * Handles Tauri errors, database errors, and standard Error objects.
 */
export function extractErrorMessage(error: unknown): string {
	if (error instanceof Error) {
		return error.message;
	}

	if (typeof error === 'string') {
		// Handle common database error formats
		const dbErrorMatch = error.match(/error returned from database: (.+)/);
		if (dbErrorMatch) {
			return dbErrorMatch[1];
		}
		return error;
	}

	if (typeof error === 'object' && error !== null) {
		const errObj = error as Record<string, unknown>;
		if (typeof errObj.message === 'string') {
			return errObj.code ? `${errObj.code}: ${errObj.message}` : errObj.message;
		}
	}

	return 'An unknown error occurred';
}
