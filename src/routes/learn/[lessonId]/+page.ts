import { LESSONS } from '$lib/tutorial/lessons';

export function entries() {
	return Object.keys(LESSONS).map((lessonId) => ({ lessonId }));
}
