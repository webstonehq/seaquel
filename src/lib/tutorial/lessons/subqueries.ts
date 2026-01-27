// src/lib/tutorial/lessons/subqueries.ts
import type { TutorialLesson } from '$lib/types';
import { criterion, sqlContains } from '../criteria';

export const subqueriesLesson: TutorialLesson = {
	id: 'subqueries',
	title: 'Subqueries',
	introduction: `
A subquery is a query nested inside another query. Subqueries can appear in:

- **WHERE clause**: Filter based on another query's results
- **FROM clause**: Use a query's results as a table
- **SELECT clause**: Calculate values for each row

Common patterns:
\`\`\`sql
-- Find products priced above average
SELECT * FROM products
WHERE price > (SELECT AVG(price) FROM products)

-- Find customers who have placed orders
SELECT * FROM customers
WHERE id IN (SELECT customer_id FROM orders)
\`\`\`

Subqueries are powerful but can be slow on large datasets. Later, you'll learn about CTEs as an alternative.
  `.trim(),
	challenges: [
		{
			id: 'sub-1',
			title: 'Above Average',
			description: 'Find all products with a price above the average price.',
			hint: 'Write: SELECT * FROM products WHERE price > (SELECT AVG(price) FROM products)',
			criteria: [
				criterion('has-subquery', 'Use a subquery with SELECT inside', sqlContains('(SELECT')),
				criterion('has-avg', 'Calculate AVG(price) in subquery', sqlContains('AVG(')),
				criterion('has-products', 'Query the products table', sqlContains('products'))
			]
		},
		{
			id: 'sub-2',
			title: 'Customers with Orders',
			description: 'Find all customers who have placed at least one order.',
			hint: 'Write: SELECT * FROM customers WHERE id IN (SELECT customer_id FROM orders)',
			criteria: [
				criterion('has-subquery', 'Use a subquery', sqlContains('(SELECT')),
				criterion('has-in', 'Use IN with the subquery', sqlContains('IN')),
				criterion('has-customers', 'Query the customers table', sqlContains('customers')),
				criterion('has-orders', 'Reference the orders table', sqlContains('orders'))
			]
		},
		{
			id: 'sub-3',
			title: 'Most Expensive Product',
			description: 'Find the product(s) with the highest price using a subquery.',
			hint: 'Write: SELECT * FROM products WHERE price = (SELECT MAX(price) FROM products)',
			criteria: [
				criterion('has-subquery', 'Use a subquery', sqlContains('(SELECT')),
				criterion('has-max', 'Use MAX(price) in subquery', sqlContains('MAX(')),
				criterion('has-products', 'Query the products table', sqlContains('products'))
			]
		},
		{
			id: 'sub-4',
			title: 'Products Never Ordered',
			description: 'Find products that have never been included in any order.',
			hint: 'Write: SELECT * FROM products WHERE id NOT IN (SELECT product_id FROM order_items)',
			criteria: [
				criterion('has-subquery', 'Use a subquery', sqlContains('(SELECT')),
				criterion('has-not-in', 'Use NOT IN', sqlContains('NOT IN')),
				criterion('has-products', 'Query the products table', sqlContains('products')),
				criterion('has-order-items', 'Reference order_items', sqlContains('order_items'))
			]
		},
		{
			id: 'sub-5',
			title: 'Scalar Subquery',
			description:
				'Show each product name and how much its price differs from the average price.',
			hint: 'Write: SELECT name, price - (SELECT AVG(price) FROM products) AS diff FROM products',
			criteria: [
				criterion('has-subquery', 'Use a subquery in SELECT', sqlContains('(SELECT')),
				criterion('has-avg', 'Calculate AVG(price)', sqlContains('AVG(')),
				criterion('has-name', 'Select the name column', sqlContains('name')),
				criterion('has-products', 'Query the products table', sqlContains('products'))
			]
		}
	]
};
