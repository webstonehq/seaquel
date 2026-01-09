export {
	type AppError,
	type ErrorCode,
	type Result,
	createError,
	extractErrorMessage,
	ok,
	err
} from './types';

export { handleError, withErrorHandling, type HandleErrorOptions } from './handler';
