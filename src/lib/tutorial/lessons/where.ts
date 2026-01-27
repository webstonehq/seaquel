// src/lib/tutorial/lessons/where.ts
import type { TutorialLesson } from '$lib/types';
import {
	criterion,
	hasTable,
	hasColumn,
	hasAnyColumns,
	hasFilter,
	hasAnyFilter
} from '../criteria';

export const whereLesson: TutorialLesson = {
	id: 'where',
	title: 'Filtering with WHERE',
	introduction: `
The WHERE clause filters which rows are returned from a query. Without WHERE, a query returns all rows from a table.

With WHERE, you specify conditions that rows must meet:
- **Comparison operators**: =, !=, >, <, >=, <=
- **Pattern matching**: LIKE for text patterns
- **Null checks**: IS NULL, IS NOT NULL
- **Lists**: IN for matching multiple values
- **Ranges**: BETWEEN for value ranges

You can combine multiple conditions using **AND** (all must be true) or **OR** (any must be true).

In the query builder, use the filter panel below the canvas to add WHERE conditions.
  `.trim(),
	challenges: [
		{
			id: 'where-1',
			title: 'Your First Filter',
			description:
				'Add the products table and create a filter to show only products where the price equals 29.99.',
			hint: 'Drag products onto the canvas, select some columns, then use the WHERE section in the filter panel to add: products.price = 29.99',
			criteria: [
				criterion('has-products', 'Add the products table', hasTable('products')),
				criterion('has-columns', 'Select at least one column', hasAnyColumns()),
				criterion(
					'has-filter',
					'Add a filter: price = 29.99',
					hasFilter('products.price', '=', '29.99')
				)
			]
		},
		{
			id: 'where-2',
			title: 'Greater Than',
			description: 'Find all products with a price greater than $100.',
			hint: 'Change the operator to ">" and set the value to 100.',
			criteria: [
				criterion('has-products', 'Add the products table', hasTable('products')),
				criterion('has-columns', 'Select at least one column', hasAnyColumns()),
				criterion(
					'has-filter',
					'Add a filter: price > 100',
					hasFilter('products.price', '>', '100')
				)
			]
		},
		{
			id: 'where-3',
			title: 'Filter by Text',
			description:
				'Find customers from the USA. Add the customers table and filter where country equals "USA".',
			hint: 'Add the customers table, select columns, then filter on customers.country = USA',
			criteria: [
				criterion('has-customers', 'Add the customers table', hasTable('customers')),
				criterion('has-columns', 'Select at least one column', hasAnyColumns()),
				criterion(
					'has-filter',
					'Add a filter: country = USA',
					hasFilter('customers.country', '=', 'USA')
				)
			]
		},
		{
			id: 'where-4',
			title: 'Low Stock Alert',
			description: 'Find products with stock less than or equal to 10.',
			hint: 'Use the <= operator on products.stock with value 10.',
			criteria: [
				criterion('has-products', 'Add the products table', hasTable('products')),
				criterion('has-name', 'Select the name column', hasColumn('products', 'name')),
				criterion('has-stock', 'Select the stock column', hasColumn('products', 'stock')),
				criterion(
					'has-filter',
					'Add a filter: stock <= 10',
					hasFilter('products.stock', '<=', '10')
				)
			]
		},
		{
			id: 'where-5',
			title: 'Combine Conditions',
			description:
				'Find products with price greater than $50 AND stock greater than 20. You\'ll need two filter conditions.',
			hint: 'Add two filters: one for price > 50 and another for stock > 20. Make sure they are connected with AND.',
			criteria: [
				criterion('has-products', 'Add the products table', hasTable('products')),
				criterion('has-columns', 'Select at least one column', hasAnyColumns()),
				criterion(
					'has-price-filter',
					'Add a filter: price > 50',
					hasFilter('products.price', '>', '50')
				),
				criterion(
					'has-stock-filter',
					'Add a filter: stock > 20',
					hasFilter('products.stock', '>', '20')
				)
			]
		}
	]
};
