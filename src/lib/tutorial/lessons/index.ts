// src/lib/tutorial/lessons/index.ts
export { introLesson } from './intro';
export { selectLesson } from './select';
export { whereLesson } from './where';
export { orderByLesson } from './order-by';
export { joinsLesson } from './joins';
export { aggregatesLesson } from './aggregates';
export { groupByLesson } from './group-by';
export { havingLesson } from './having';
export { subqueriesLesson } from './subqueries';
export { cteLesson } from './cte';
export { windowLesson } from './window';

import type { TutorialLesson } from '$lib/types';
import { introLesson } from './intro';
import { selectLesson } from './select';
import { whereLesson } from './where';
import { orderByLesson } from './order-by';
import { joinsLesson } from './joins';
import { aggregatesLesson } from './aggregates';
import { groupByLesson } from './group-by';
import { havingLesson } from './having';
import { subqueriesLesson } from './subqueries';
import { cteLesson } from './cte';
import { windowLesson } from './window';

export const LESSONS: Record<string, TutorialLesson> = {
	// SQL Basics
	intro: introLesson,
	select: selectLesson,
	where: whereLesson,
	'order-by': orderByLesson,

	// Intermediate SQL
	joins: joinsLesson,
	aggregates: aggregatesLesson,
	'group-by': groupByLesson,
	having: havingLesson,

	// Advanced SQL
	subqueries: subqueriesLesson,
	cte: cteLesson,
	window: windowLesson,
};

export function getLesson(id: string): TutorialLesson | undefined {
	return LESSONS[id];
}

/**
 * Get the next lesson ID in the sequence, or undefined if this is the last lesson.
 */
export function getNextLessonId(currentId: string): string | undefined {
	const allLessonIds: string[] = LESSON_SECTIONS.flatMap((section) => [...section.lessons]);
	const currentIndex = allLessonIds.indexOf(currentId);
	if (currentIndex === -1 || currentIndex === allLessonIds.length - 1) {
		return undefined;
	}
	return allLessonIds[currentIndex + 1];
}

/**
 * Lesson sections for organizing the tutorial sidebar and learn page.
 * Each section contains a group of related lessons.
 */
export const LESSON_SECTIONS: readonly { id: string; title: string; lessons: readonly string[] }[] = [
	{
		id: 'basics',
		title: 'SQL Basics',
		lessons: ['intro', 'select', 'where', 'order-by']
	},
	{
		id: 'intermediate',
		title: 'Intermediate SQL',
		lessons: ['joins', 'aggregates', 'group-by', 'having']
	},
	{
		id: 'advanced',
		title: 'Advanced SQL',
		lessons: ['subqueries', 'cte', 'window']
	}
];
