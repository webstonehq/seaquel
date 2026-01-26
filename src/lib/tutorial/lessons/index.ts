// src/lib/tutorial/lessons/index.ts
export { selectLesson } from './select';

import type { TutorialLesson } from '$lib/types';
import { selectLesson } from './select';

export const LESSONS: Record<string, TutorialLesson> = {
	select: selectLesson
};

export function getLesson(id: string): TutorialLesson | undefined {
	return LESSONS[id];
}
