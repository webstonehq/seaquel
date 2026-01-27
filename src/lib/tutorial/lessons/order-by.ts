// src/lib/tutorial/lessons/order-by.ts
import type { TutorialLesson } from '$lib/types';
import {
	criterion,
	hasTable,
	hasColumn,
	hasAnyColumns,
	hasOrderBy,
	hasAnyOrderBy,
	hasLimit
} from '../criteria';

export const orderByLesson: TutorialLesson = {
	id: 'order-by',
	title: 'Sorting with ORDER BY',
	introduction: `
ORDER BY sorts the results of your query. By default, SQL doesn't guarantee any particular order, so if you need sorted results, you must specify it.

You can sort:
- **ASC** (ascending) - smallest to largest, A to Z (this is the default)
- **DESC** (descending) - largest to smallest, Z to A

You can also sort by multiple columns. The first column is the primary sort, and subsequent columns break ties.

LIMIT restricts how many rows are returned. Combined with ORDER BY, this is powerful for finding "top N" results.

Use the ORDER BY section in the filter panel to add sorting.
  `.trim(),
	challenges: [
		{
			id: 'order-1',
			title: 'Alphabetical Order',
			description: 'List all customers sorted by name in ascending (A-Z) order.',
			hint: 'Add the customers table, select the name column, then add ORDER BY customers.name ASC.',
			criteria: [
				criterion('has-customers', 'Add the customers table', hasTable('customers')),
				criterion('has-name', 'Select the name column', hasColumn('customers', 'name')),
				criterion('has-order', 'Sort by name ascending', hasOrderBy('customers.name', 'ASC'))
			]
		},
		{
			id: 'order-2',
			title: 'Highest Price First',
			description: 'Show products sorted by price from highest to lowest.',
			hint: 'Use ORDER BY products.price DESC to sort in descending order.',
			criteria: [
				criterion('has-products', 'Add the products table', hasTable('products')),
				criterion('has-name', 'Select the name column', hasColumn('products', 'name')),
				criterion('has-price', 'Select the price column', hasColumn('products', 'price')),
				criterion('has-order', 'Sort by price descending', hasOrderBy('products.price', 'DESC'))
			]
		},
		{
			id: 'order-3',
			title: 'Top 3 Products',
			description: 'Show the 3 most expensive products.',
			hint: 'Combine ORDER BY price DESC with LIMIT 3.',
			criteria: [
				criterion('has-products', 'Add the products table', hasTable('products')),
				criterion('has-columns', 'Select at least one column', hasAnyColumns()),
				criterion('has-order', 'Sort by price descending', hasOrderBy('products.price', 'DESC')),
				criterion('has-limit', 'Limit to 3 results', hasLimit(3))
			]
		},
		{
			id: 'order-4',
			title: 'Newest Customers',
			description: 'Find the 5 most recently created customer accounts.',
			hint: 'Sort by created_at in descending order and limit to 5.',
			criteria: [
				criterion('has-customers', 'Add the customers table', hasTable('customers')),
				criterion('has-name', 'Select the name column', hasColumn('customers', 'name')),
				criterion(
					'has-created',
					'Select the created_at column',
					hasColumn('customers', 'created_at')
				),
				criterion(
					'has-order',
					'Sort by created_at descending',
					hasOrderBy('customers.created_at', 'DESC')
				),
				criterion('has-limit', 'Limit to 5 results', hasLimit(5))
			]
		},
		{
			id: 'order-5',
			title: 'Cheapest In-Stock Items',
			description: 'Show the 10 cheapest products that have stock greater than 0.',
			hint: 'Add a filter for stock > 0, then ORDER BY price ASC with LIMIT 10.',
			criteria: [
				criterion('has-products', 'Add the products table', hasTable('products')),
				criterion('has-name', 'Select the name column', hasColumn('products', 'name')),
				criterion('has-price', 'Select the price column', hasColumn('products', 'price')),
				criterion('has-order', 'Sort by price ascending', hasOrderBy('products.price', 'ASC')),
				criterion('has-limit', 'Limit to 10 results', hasLimit(10))
			]
		}
	]
};
