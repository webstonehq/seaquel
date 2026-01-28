// src/lib/tutorial/lessons/cte.ts
import type { TutorialLesson } from '$lib/types';
import { criterion, sqlContains } from '../criteria';

export const cteLesson: TutorialLesson = {
	id: 'cte',
	title: 'Common Table Expressions',
	introduction: `
A Common Table Expression (CTE) is a temporary named result set that exists only within a single query. CTEs make complex queries more readable.

Basic syntax:
\`\`\`sql
WITH cte_name AS (
    SELECT ...
)
SELECT * FROM cte_name
\`\`\`

Benefits over subqueries:
- **Readable**: Name your intermediate results
- **Reusable**: Reference the CTE multiple times
- **Recursive**: CTEs can reference themselves for hierarchical data

Think of CTEs as creating temporary "views" for your query.
  `.trim(),
	challenges: [
		{
			id: 'cte-1',
			title: 'Your First CTE',
			description:
				'Create a CTE called "expensive_products" that selects products over $50, then select from it.',
			hint: 'Write: WITH expensive_products AS (SELECT * FROM products WHERE price > 50) SELECT * FROM expensive_products',
			criteria: [
				criterion('has-with', 'Start with WITH keyword', sqlContains('WITH')),
				criterion('has-as', 'Use AS to define the CTE', sqlContains('AS')),
				criterion('has-cte-name', 'Name your CTE', sqlContains('expensive_products')),
				criterion('has-products', 'Query products table', sqlContains('products')),
				criterion('has-where', 'Use WHERE to filter', sqlContains('WHERE')),
				criterion('has-price-filter', 'Filter by price > 50', (_, sql) => {
					// Must have price > 50 comparison
					return /price\s*>\s*50/i.test(sql);
				})
			]
		},
		{
			id: 'cte-2',
			title: 'CTE with Aggregation',
			description:
				'Create a CTE that calculates the average order total, then find orders above that average.',
			hint: 'Write: WITH avg_order AS (SELECT AVG(total) as avg_total FROM orders) SELECT * FROM orders, avg_order WHERE total > avg_total',
			criteria: [
				criterion('has-with', 'Start with WITH keyword', sqlContains('WITH')),
				criterion('has-avg', 'Calculate AVG() in CTE', sqlContains('AVG(')),
				criterion('has-orders', 'Query the orders table', sqlContains('orders')),
				criterion('has-where', 'Use WHERE to filter', sqlContains('WHERE')),
				criterion('has-comparison', 'Compare orders to the average using >', (_, sql) => {
					// Must have a greater-than comparison (total > avg or similar)
					return sql.includes('>');
				})
			]
		},
		{
			id: 'cte-3',
			title: 'Multiple CTEs',
			description:
				'Create two CTEs: one for product counts by category, another for the average count. Then find categories above average.',
			hint: 'Use comma to separate CTEs: WITH cte1 AS (...), cte2 AS (...) SELECT ...',
			criteria: [
				criterion('has-with', 'Start with WITH keyword', sqlContains('WITH')),
				criterion('has-count', 'Use COUNT() in a CTE', sqlContains('COUNT(')),
				criterion('has-avg', 'Use AVG() to find average', sqlContains('AVG(')),
				criterion('has-category', 'Reference category_id', sqlContains('category_id')),
				criterion('has-where', 'Use WHERE to filter results', sqlContains('WHERE')),
				criterion('has-comparison', 'Compare to average using >', (_, sql) => {
					return sql.includes('>');
				})
			]
		},
		{
			id: 'cte-4',
			title: 'CTE for Readability',
			description:
				'Use a CTE to first find all orders from 2024, then calculate their total value.',
			hint: 'Write: WITH recent_orders AS (SELECT * FROM orders WHERE created_at >= "2024-01-01") SELECT SUM(total) FROM recent_orders',
			criteria: [
				criterion('has-with', 'Start with WITH keyword', sqlContains('WITH')),
				criterion('has-sum', 'Use SUM() for total', sqlContains('SUM(')),
				criterion('has-orders', 'Query orders table', sqlContains('orders')),
				criterion('has-where', 'Use WHERE to filter by date', sqlContains('WHERE')),
				criterion('has-2024', 'Filter for 2024', sqlContains('2024'))
			]
		},
		{
			id: 'cte-5',
			title: 'Customer Order Summary',
			description:
				'Create a CTE that calculates each customer\'s total spending, then find customers who spent over $200.',
			hint: 'Write: WITH customer_totals AS (SELECT customer_id, SUM(total) as total_spent FROM orders GROUP BY customer_id) SELECT * FROM customer_totals WHERE total_spent > 200',
			criteria: [
				criterion('has-with', 'Start with WITH keyword', sqlContains('WITH')),
				criterion('has-sum', 'Use SUM() for totals', sqlContains('SUM(')),
				criterion('has-group-by', 'Use GROUP BY', sqlContains('GROUP BY')),
				criterion('has-customer', 'Reference customer_id', sqlContains('customer_id')),
				criterion('has-where', 'Use WHERE to filter', sqlContains('WHERE')),
				criterion('has-200-filter', 'Filter for spending > 200', (_, sql) => {
					// Must have > 200 comparison
					return />\s*200/i.test(sql);
				})
			]
		}
	]
};
