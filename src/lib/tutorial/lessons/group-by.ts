// src/lib/tutorial/lessons/group-by.ts
import type { TutorialLesson } from '$lib/types';
import {
	criterion,
	hasTable,
	hasColumn,
	hasAnyColumns,
	hasGroupBy,
	hasAnyGroupBy,
	sqlContains
} from '../criteria';

export const groupByLesson: TutorialLesson = {
	id: 'group-by',
	title: 'Grouping with GROUP BY',
	introduction: `
GROUP BY divides rows into groups based on column values. When combined with aggregate functions, it lets you calculate summaries for each group.

For example, to count products per category:
\`\`\`sql
SELECT category_id, COUNT(*)
FROM products
GROUP BY category_id
\`\`\`

Rules for GROUP BY:
- Every column in SELECT must either be in GROUP BY or inside an aggregate function
- GROUP BY comes after WHERE but before ORDER BY
- You can group by multiple columns

In the query builder, use the GROUP BY section in the filter panel to add grouping columns.
  `.trim(),
	challenges: [
		{
			id: 'group-1',
			title: 'Your First Grouping',
			description:
				'Add the products table and group by category_id. Select the category_id column to see which categories exist.',
			hint: 'Drag products onto the canvas, select category_id, then use the GROUP BY section in the filter panel to add products.category_id.',
			criteria: [
				criterion('has-products', 'Add the products table', hasTable('products')),
				criterion(
					'has-category-col',
					'Select the category_id column',
					hasColumn('products', 'category_id')
				),
				criterion(
					'has-group-by',
					'Group by products.category_id',
					hasGroupBy('products.category_id')
				)
			]
		},
		{
			id: 'group-2',
			title: 'Customers by Country',
			description: 'Group customers by their country to see how many countries you have customers in.',
			hint: 'Add the customers table, select the country column, then add GROUP BY customers.country.',
			criteria: [
				criterion('has-customers', 'Add the customers table', hasTable('customers')),
				criterion('has-country-col', 'Select the country column', hasColumn('customers', 'country')),
				criterion('has-group-by', 'Group by customers.country', hasGroupBy('customers.country'))
			]
		},
		{
			id: 'group-3',
			title: 'Products by Category',
			description: 'Show all products grouped by category, displaying both the category_id and product name.',
			hint: 'Add products, select category_id and name columns, then GROUP BY products.category_id.',
			criteria: [
				criterion('has-products', 'Add the products table', hasTable('products')),
				criterion(
					'has-category-col',
					'Select the category_id column',
					hasColumn('products', 'category_id')
				),
				criterion('has-name-col', 'Select the name column', hasColumn('products', 'name')),
				criterion(
					'has-group-by',
					'Group by products.category_id',
					hasGroupBy('products.category_id')
				)
			]
		},
		{
			id: 'group-4',
			title: 'Orders by Status',
			description: 'Group orders by their status to analyze order states.',
			hint: 'Add orders, select the status column, then GROUP BY orders.status.',
			criteria: [
				criterion('has-orders', 'Add the orders table', hasTable('orders')),
				criterion('has-status-col', 'Select the status column', hasColumn('orders', 'status')),
				criterion('has-group-by', 'Group by orders.status', hasGroupBy('orders.status'))
			]
		},
		{
			id: 'group-5',
			title: 'Multiple Grouping Columns',
			description:
				'Group orders by both customer_id and status. This creates groups for each unique combination.',
			hint: 'Add orders, select customer_id and status, then add two GROUP BY clauses: orders.customer_id and orders.status.',
			criteria: [
				criterion('has-orders', 'Add the orders table', hasTable('orders')),
				criterion(
					'has-customer-col',
					'Select the customer_id column',
					hasColumn('orders', 'customer_id')
				),
				criterion('has-status-col', 'Select the status column', hasColumn('orders', 'status')),
				criterion(
					'has-group-by-customer',
					'Group by orders.customer_id',
					hasGroupBy('orders.customer_id')
				),
				criterion('has-group-by-status', 'Group by orders.status', hasGroupBy('orders.status'))
			]
		}
	]
};
