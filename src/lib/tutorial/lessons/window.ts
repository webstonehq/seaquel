// src/lib/tutorial/lessons/window.ts
import type { TutorialLesson } from '$lib/types';
import { criterion, sqlContains } from '../criteria';

export const windowLesson: TutorialLesson = {
	id: 'window',
	title: 'Window Functions',
	introduction: `
Window functions perform calculations across rows related to the current row, without collapsing them into groups like GROUP BY does.

Syntax:
\`\`\`sql
function_name() OVER (
    PARTITION BY column  -- optional: divide into groups
    ORDER BY column      -- optional: define row order
)
\`\`\`

Common window functions:
- **ROW_NUMBER()** - Sequential row number
- **RANK()** - Rank with gaps for ties
- **DENSE_RANK()** - Rank without gaps
- **SUM/AVG/COUNT() OVER** - Running or partitioned aggregates
- **LAG/LEAD()** - Access previous/next row values

Window functions are powerful for rankings, running totals, and comparisons.
  `.trim(),
	challenges: [
		{
			id: 'window-1',
			title: 'Row Numbers',
			description: 'Add row numbers to all products ordered by price descending.',
			hint: 'Write: SELECT name, price, ROW_NUMBER() OVER (ORDER BY price DESC) as rank FROM products',
			criteria: [
				criterion('has-row-number', 'Use ROW_NUMBER()', sqlContains('ROW_NUMBER()')),
				criterion('has-over', 'Use OVER clause', sqlContains('OVER')),
				criterion('has-order', 'ORDER BY in window', sqlContains('ORDER BY')),
				criterion('has-products', 'Query products table', sqlContains('products'))
			]
		},
		{
			id: 'window-2',
			title: 'Ranking Products',
			description: 'Rank products by price within each category using RANK().',
			hint: 'Write: SELECT name, category_id, price, RANK() OVER (PARTITION BY category_id ORDER BY price DESC) FROM products',
			criteria: [
				criterion('has-rank', 'Use RANK()', sqlContains('RANK()')),
				criterion('has-over', 'Use OVER clause', sqlContains('OVER')),
				criterion('has-partition', 'Use PARTITION BY', sqlContains('PARTITION BY')),
				criterion('has-category', 'Partition by category_id', sqlContains('category_id'))
			]
		},
		{
			id: 'window-3',
			title: 'Running Total',
			description: 'Calculate a running total of order amounts ordered by date.',
			hint: 'Write: SELECT id, total, SUM(total) OVER (ORDER BY created_at) as running_total FROM orders',
			criteria: [
				criterion('has-sum', 'Use SUM() as window function', sqlContains('SUM(')),
				criterion('has-over', 'Use OVER clause', sqlContains('OVER')),
				criterion('has-order', 'ORDER BY in window', sqlContains('ORDER BY')),
				criterion('has-orders', 'Query orders table', sqlContains('orders'))
			]
		},
		{
			id: 'window-4',
			title: 'Compare to Average',
			description:
				'For each product, show its price and the average price in its category.',
			hint: 'Write: SELECT name, price, AVG(price) OVER (PARTITION BY category_id) as category_avg FROM products',
			criteria: [
				criterion('has-avg', 'Use AVG() as window function', sqlContains('AVG(')),
				criterion('has-over', 'Use OVER clause', sqlContains('OVER')),
				criterion('has-partition', 'Use PARTITION BY', sqlContains('PARTITION BY')),
				criterion('has-products', 'Query products table', sqlContains('products'))
			]
		},
		{
			id: 'window-5',
			title: 'Dense Ranking',
			description:
				'Use DENSE_RANK to rank customers by their total order count (highest first).',
			hint: 'Combine with a subquery or CTE: count orders per customer, then apply DENSE_RANK()',
			criteria: [
				criterion('has-dense-rank', 'Use DENSE_RANK()', sqlContains('DENSE_RANK()')),
				criterion('has-over', 'Use OVER clause', sqlContains('OVER')),
				criterion('has-count', 'Count orders', sqlContains('COUNT(')),
				criterion('has-customer', 'Reference customer_id', sqlContains('customer_id'))
			]
		}
	]
};
