// src/lib/tutorial/lessons/delete.ts
import type { TutorialLesson } from '$lib/types';
import { criterion, sqlContains } from '../criteria';

export const deleteLesson: TutorialLesson = {
	id: 'delete',
	title: 'DELETE Statements',
	introduction: `
DELETE removes rows from a table.

Basic syntax:
\`\`\`sql
DELETE FROM table
WHERE condition
\`\`\`

**CRITICAL**: Always use a WHERE clause! Without it, DELETE removes ALL rows from the table.

Common patterns:
\`\`\`sql
-- Delete specific row
DELETE FROM products WHERE id = 5

-- Delete matching rows
DELETE FROM orders WHERE status = 'cancelled'

-- Delete with subquery
DELETE FROM products
WHERE id NOT IN (SELECT product_id FROM order_items)
\`\`\`

Note: In this tutorial sandbox, DELETE statements are simulated and won't permanently modify data.
  `.trim(),
	challenges: [
		{
			id: 'delete-1',
			title: 'Delete a Single Row',
			description: 'Delete the product with id 5.',
			hint: 'Write: DELETE FROM products WHERE id = 5',
			criteria: [
				criterion('has-delete', 'Use DELETE FROM', sqlContains('DELETE FROM')),
				criterion('has-where', 'Use WHERE to target specific row', sqlContains('WHERE')),
				criterion('has-products', 'Delete from products table', sqlContains('products'))
			]
		},
		{
			id: 'delete-2',
			title: 'Delete by Condition',
			description: 'Delete all products with stock equal to 0.',
			hint: 'Write: DELETE FROM products WHERE stock = 0',
			criteria: [
				criterion('has-delete', 'Use DELETE FROM', sqlContains('DELETE FROM')),
				criterion('has-where', 'Use WHERE clause', sqlContains('WHERE')),
				criterion('has-stock', 'Filter by stock', sqlContains('stock')),
				criterion('has-products', 'Delete from products table', sqlContains('products'))
			]
		},
		{
			id: 'delete-3',
			title: 'Delete Old Orders',
			description: 'Delete all orders with status "cancelled".',
			hint: 'Write: DELETE FROM orders WHERE status = \'cancelled\'',
			criteria: [
				criterion('has-delete', 'Use DELETE FROM', sqlContains('DELETE FROM')),
				criterion('has-where', 'Use WHERE clause', sqlContains('WHERE')),
				criterion('has-status', 'Filter by status', sqlContains('status')),
				criterion('has-orders', 'Delete from orders table', sqlContains('orders'))
			]
		},
		{
			id: 'delete-4',
			title: 'Delete with Multiple Conditions',
			description: 'Delete products that have price less than $10 AND stock greater than 100.',
			hint: 'Write: DELETE FROM products WHERE price < 10 AND stock > 100',
			criteria: [
				criterion('has-delete', 'Use DELETE FROM', sqlContains('DELETE FROM')),
				criterion('has-where', 'Use WHERE clause', sqlContains('WHERE')),
				criterion('has-and', 'Combine conditions with AND', sqlContains('AND')),
				criterion('has-products', 'Delete from products table', sqlContains('products'))
			]
		},
		{
			id: 'delete-5',
			title: 'Delete with Subquery',
			description: 'Delete all reviews from customers who are not in the USA.',
			hint: 'Write: DELETE FROM reviews WHERE customer_id IN (SELECT id FROM customers WHERE country != \'USA\')',
			criteria: [
				criterion('has-delete', 'Use DELETE FROM', sqlContains('DELETE FROM')),
				criterion('has-subquery', 'Use a subquery', sqlContains('(SELECT')),
				criterion('has-reviews', 'Delete from reviews table', sqlContains('reviews')),
				criterion('has-customers', 'Reference customers in subquery', sqlContains('customers'))
			]
		}
	]
};
