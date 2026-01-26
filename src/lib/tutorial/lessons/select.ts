// src/lib/tutorial/lessons/select.ts
import type { TutorialLesson } from '$lib/types';
import {
	criterion,
	hasTable,
	hasColumn,
	hasAnyColumns,
	hasFilter,
	hasOrderBy,
	hasLimit
} from '../criteria';

export const selectLesson: TutorialLesson = {
	id: 'select',
	title: 'SELECT Statements',
	introduction: `
The SELECT statement is the most fundamental SQL command. It retrieves data from one or more tables in a database.

Every SELECT query has at least two parts:
- **SELECT** - specifies which columns you want to retrieve
- **FROM** - specifies which table to get the data from

In this lesson, you'll learn to build SELECT queries step by step using the visual query builder. Drag tables onto the canvas, check the columns you want, and watch the SQL update in real-time.
  `.trim(),
	challenges: [
		{
			id: 'select-1',
			title: 'Your First Query',
			description:
				'Drag the "products" table onto the canvas and select the "name" and "price" columns.',
			hint: 'Look for the products table in the left panel. Drag it onto the canvas, then check the boxes next to "name" and "price".',
			criteria: [
				criterion('has-products', 'Add the products table to the canvas', hasTable('products')),
				criterion('has-name', 'Select the name column', hasColumn('products', 'name')),
				criterion('has-price', 'Select the price column', hasColumn('products', 'price'))
			]
		},
		{
			id: 'select-2',
			title: 'Select All Columns',
			description:
				'Sometimes you want all columns from a table. Add the "customers" table and select all of its columns.',
			hint: 'Use the "All" button in the table header to select all columns at once.',
			criteria: [
				criterion('has-customers', 'Add the customers table', hasTable('customers')),
				criterion('has-id', 'Select the id column', hasColumn('customers', 'id')),
				criterion('has-name', 'Select the name column', hasColumn('customers', 'name')),
				criterion('has-email', 'Select the email column', hasColumn('customers', 'email')),
				criterion('has-country', 'Select the country column', hasColumn('customers', 'country')),
				criterion(
					'has-created',
					'Select the created_at column',
					hasColumn('customers', 'created_at')
				)
			]
		},
		{
			id: 'select-3',
			title: 'Filtering Results',
			description:
				'Add a WHERE clause to filter products. Show only products with a price greater than $50.',
			hint: 'Use the WHERE section in the filter panel below the canvas. Add a filter on products.price with the "greater than" operator.',
			criteria: [
				criterion('has-products', 'Add the products table', hasTable('products')),
				criterion('has-columns', 'Select at least one column', hasAnyColumns()),
				criterion(
					'has-filter',
					'Add a WHERE filter on price > 50',
					hasFilter('products.price', '>', '50')
				)
			]
		},
		{
			id: 'select-4',
			title: 'Sorting Results',
			description: 'Sort the products by price in descending order (highest first).',
			hint: 'Use the ORDER BY section in the filter panel. Add products.price and set direction to DESC.',
			criteria: [
				criterion('has-products', 'Add the products table', hasTable('products')),
				criterion('has-columns', 'Select at least one column', hasAnyColumns()),
				criterion('has-order', 'Sort by price descending', hasOrderBy('products.price', 'DESC'))
			]
		},
		{
			id: 'select-5',
			title: 'Limiting Results',
			description: 'Show only the top 5 most expensive products.',
			hint: 'Combine ORDER BY (price DESC) with LIMIT 5 to get just the top 5.',
			criteria: [
				criterion('has-products', 'Add the products table', hasTable('products')),
				criterion('has-name', 'Select the name column', hasColumn('products', 'name')),
				criterion('has-price', 'Select the price column', hasColumn('products', 'price')),
				criterion('has-order', 'Sort by price descending', hasOrderBy('products.price', 'DESC')),
				criterion('has-limit', 'Limit to 5 results', hasLimit(5))
			]
		}
	]
};
