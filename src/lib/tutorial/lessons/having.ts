// src/lib/tutorial/lessons/having.ts
import type { TutorialLesson } from '$lib/types';
import {
	criterion,
	hasTable,
	hasGroupBy,
	hasHavingComparison,
	hasFilter
} from '../criteria';

export const havingLesson: TutorialLesson = {
	id: 'having',
	title: 'Filtering Groups with HAVING',
	introduction: `
HAVING filters groups after GROUP BY, just like WHERE filters rows before grouping.

Use WHERE for filtering individual rows:
\`\`\`sql
SELECT category_id, COUNT(*) FROM products
WHERE price > 50
GROUP BY category_id
\`\`\`

Use HAVING for filtering groups:
\`\`\`sql
SELECT category_id, COUNT(*) FROM products
GROUP BY category_id
HAVING COUNT(*) > 5
\`\`\`

You can use both together:
- WHERE filters rows before grouping
- GROUP BY creates groups
- HAVING filters groups after aggregation
  `.trim(),
	challenges: [
		{
			id: 'having-1',
			title: 'Popular Categories',
			description: 'Find categories that have more than 3 products.',
			hint: 'Write: SELECT category_id, COUNT(*) FROM products GROUP BY category_id HAVING COUNT(*) > 3',
			criteria: [
				criterion('has-products', 'Query the products table', hasTable('products')),
				criterion(
					'has-group-by',
					'Group by products.category_id',
					hasGroupBy('products.category_id')
				),
				criterion(
					'has-having',
					'HAVING COUNT(*) > 3',
					hasHavingComparison('COUNT', '>', '3')
				)
			]
		},
		{
			id: 'having-2',
			title: 'High-Value Categories',
			description: 'Find categories where the average product price is over $75.',
			hint: 'Write: SELECT category_id, AVG(price) FROM products GROUP BY category_id HAVING AVG(price) > 75',
			criteria: [
				criterion('has-products', 'Query the products table', hasTable('products')),
				criterion(
					'has-group-by',
					'Group by products.category_id',
					hasGroupBy('products.category_id')
				),
				criterion(
					'has-having',
					'HAVING AVG(...) > 75',
					hasHavingComparison('AVG', '>', '75')
				)
			]
		},
		{
			id: 'having-3',
			title: 'Active Countries',
			description: 'Find countries with at least 2 customers.',
			hint: 'Write: SELECT country, COUNT(*) FROM customers GROUP BY country HAVING COUNT(*) >= 2',
			criteria: [
				criterion('has-customers', 'Query the customers table', hasTable('customers')),
				criterion(
					'has-group-by',
					'Group by customers.country',
					hasGroupBy('customers.country')
				),
				criterion(
					'has-having',
					'HAVING COUNT(*) >= 2',
					hasHavingComparison('COUNT', '>=', '2')
				)
			]
		},
		{
			id: 'having-4',
			title: 'Combine WHERE and HAVING',
			description:
				'Find categories with more than 2 products that cost over $20. Use WHERE to filter products by price first.',
			hint: 'Write: SELECT category_id, COUNT(*) FROM products WHERE price > 20 GROUP BY category_id HAVING COUNT(*) > 2',
			criteria: [
				criterion('has-products', 'Query the products table', hasTable('products')),
				criterion(
					'has-where',
					'WHERE price > 20',
					hasFilter('products.price', '>', '20')
				),
				criterion(
					'has-group-by',
					'Group by products.category_id',
					hasGroupBy('products.category_id')
				),
				criterion(
					'has-having',
					'HAVING COUNT(*) > 2',
					hasHavingComparison('COUNT', '>', '2')
				)
			]
		},
		{
			id: 'having-5',
			title: 'High-Spending Status',
			description: 'Find order statuses where the total order value exceeds $500.',
			hint: 'Write: SELECT status, SUM(total) FROM orders GROUP BY status HAVING SUM(total) > 500',
			criteria: [
				criterion('has-orders', 'Query the orders table', hasTable('orders')),
				criterion('has-group-by', 'Group by orders.status', hasGroupBy('orders.status')),
				criterion(
					'has-having',
					'HAVING SUM(...) > 500',
					hasHavingComparison('SUM', '>', '500')
				)
			]
		}
	]
};
