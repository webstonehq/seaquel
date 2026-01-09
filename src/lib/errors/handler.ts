/**
 * Centralized error handling with severity-based routing.
 * Provides consistent user feedback and logging across the application.
 */

import { toast } from 'svelte-sonner';
import type { AppError, ErrorCode } from './types';

type ErrorSeverity = 'error' | 'warning' | 'info';

/**
 * Maps error codes to their display severity.
 * Determines whether to show error toast, warning, or info.
 */
const errorSeverity: Record<ErrorCode, ErrorSeverity> = {
	CONNECTION_FAILED: 'error',
	QUERY_FAILED: 'error',
	PERSISTENCE_FAILED: 'warning', // Don't block user for persistence issues
	SSH_TUNNEL_FAILED: 'error',
	SCHEMA_LOAD_FAILED: 'error',
	VALIDATION_FAILED: 'warning',
	EXPORT_FAILED: 'error',
	IMPORT_FAILED: 'error',
	UNKNOWN: 'error'
};

export interface HandleErrorOptions {
	/** If true, don't show a toast notification */
	silent?: boolean;
	/** Override the default severity */
	severity?: ErrorSeverity;
}

/**
 * Handles an AppError by logging and optionally showing a toast notification.
 *
 * @example
 * // Show error toast
 * handleError(appError);
 *
 * // Silent error (logging only)
 * handleError(appError, { silent: true });
 */
export function handleError(error: AppError, options?: HandleErrorOptions): void {
	// Always log for debugging
	console.error(`[${error.code}] ${error.message}`, error.context, error.cause);

	if (options?.silent) {
		return;
	}

	const severity = options?.severity ?? errorSeverity[error.code];

	switch (severity) {
		case 'error':
			toast.error(error.userMessage);
			break;
		case 'warning':
			toast.warning(error.userMessage);
			break;
		case 'info':
			toast.info(error.userMessage);
			break;
	}
}

/**
 * Wraps an async operation with error handling.
 * Returns a Result type for explicit error handling.
 *
 * @example
 * const result = await withErrorHandling(
 *   () => fetchData(),
 *   'QUERY_FAILED',
 *   'Failed to fetch data'
 * );
 */
export async function withErrorHandling<T>(
	operation: () => Promise<T>,
	errorCode: ErrorCode,
	userMessage: string,
	options?: HandleErrorOptions & { context?: Record<string, unknown> }
): Promise<{ ok: true; value: T } | { ok: false; error: AppError }> {
	try {
		const value = await operation();
		return { ok: true, value };
	} catch (e) {
		const error: AppError = {
			code: errorCode,
			message: e instanceof Error ? e.message : String(e),
			userMessage,
			context: options?.context,
			cause: e instanceof Error ? e : undefined
		};

		handleError(error, options);
		return { ok: false, error };
	}
}
