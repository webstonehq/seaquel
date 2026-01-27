// src/lib/tutorial/lessons/intro.ts
import type { TutorialLesson } from '$lib/types';
import { criterion, hasTable, hasColumn, hasAnyColumns } from '../criteria';

export const introLesson: TutorialLesson = {
	id: 'intro',
	title: 'Introduction to SQL',
	introduction: `
Welcome to SQL! SQL (Structured Query Language) is the standard language for working with databases.

In this tutorial, you'll learn SQL using a visual query builder. Instead of typing queries by hand, you'll:
- **Drag tables** onto the canvas to add them to your query
- **Check columns** to select which data to retrieve
- **Connect tables** to join related data together
- **Add filters** to narrow down results

The query builder shows the SQL code in real-time as you build your query visually. This helps you understand how SQL works while getting immediate feedback.

Let's start with the basics!
  `.trim(),
	challenges: [
		{
			id: 'intro-1',
			title: 'Explore the Interface',
			description:
				'Take a look around! On the left, you\'ll see a list of available tables. Drag the "categories" table onto the canvas to begin.',
			hint: 'Find the "categories" table in the left panel and drag it onto the gray canvas area.',
			criteria: [
				criterion(
					'has-categories',
					'Drag the categories table onto the canvas',
					hasTable('categories')
				)
			]
		},
		{
			id: 'intro-2',
			title: 'Select a Column',
			description:
				'Great! Now select the "name" column from the categories table by clicking the checkbox next to it.',
			hint: 'Look at the categories table on the canvas. Click the checkbox next to "name" to select it.',
			criteria: [
				criterion('has-categories', 'Add the categories table', hasTable('categories')),
				criterion('has-name', 'Select the name column', hasColumn('categories', 'name'))
			]
		},
		{
			id: 'intro-3',
			title: 'Select Multiple Columns',
			description:
				'You can select multiple columns. Add the "description" column as well to see how the SQL changes.',
			hint: 'Check the box next to "description" in the categories table.',
			criteria: [
				criterion('has-categories', 'Add the categories table', hasTable('categories')),
				criterion('has-name', 'Select the name column', hasColumn('categories', 'name')),
				criterion(
					'has-description',
					'Select the description column',
					hasColumn('categories', 'description')
				)
			]
		},
		{
			id: 'intro-4',
			title: 'Try Another Table',
			description:
				'Now add the "products" table to the canvas and select its "name" and "price" columns.',
			hint: 'Drag the products table onto the canvas, then check the name and price columns.',
			criteria: [
				criterion('has-products', 'Add the products table', hasTable('products')),
				criterion('has-name', 'Select the name column', hasColumn('products', 'name')),
				criterion('has-price', 'Select the price column', hasColumn('products', 'price'))
			]
		}
	]
};
