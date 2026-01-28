// src/lib/tutorial/lessons/subqueries.ts
import type { TutorialLesson } from '$lib/types';
import {
	criterion,
	sqlContains,
	hasSubquery,
	hasSubqueryWithTable,
	hasSubqueryAggregate,
	hasFilterWithSubquery,
	hasTable
} from '../criteria';

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

**Visual Builder**: You can also build subqueries visually! Drag a "Subquery" from the palette onto the canvas, then drag tables into it to build the inner query. Link it to a WHERE filter using the subquery button.
  `.trim(),
	challenges: [
		{
			id: 'sub-1',
			title: 'Above Average',
			description: 'Find all products with a price above the average price.',
			hint: 'Write: SELECT * FROM products WHERE price > (SELECT AVG(price) FROM products)\n\nOr visually: Add products table, create a WHERE subquery with AVG(price), and link it to a price filter.',
			criteria: [
				// SQL-based criteria (works with typed SQL)
				criterion('has-subquery', 'Use a subquery with SELECT inside', (state, sql) => {
					// Check SQL or visual state
					return sqlContains('(SELECT')(state, sql) || hasSubquery('where')(state, sql);
				}),
				criterion('has-avg', 'Calculate AVG(price) in subquery', (state, sql) => {
					return sqlContains('AVG(')(state, sql) || hasSubqueryAggregate('AVG')(state, sql);
				}),
				criterion('has-products', 'Query the products table', (state, sql) => {
					return sqlContains('products')(state, sql) || hasTable('products')(state, sql);
				})
			]
		},
		{
			id: 'sub-2',
			title: 'Customers with Orders',
			description: 'Find all customers who have placed at least one order.',
			hint: 'Write: SELECT * FROM customers WHERE id IN (SELECT customer_id FROM orders)\n\nOr visually: Add customers table, create a WHERE subquery with orders table selecting customer_id, link with IN operator.',
			criteria: [
				criterion('has-subquery', 'Use a subquery', (state, sql) => {
					return sqlContains('(SELECT')(state, sql) || hasSubquery('where')(state, sql);
				}),
				criterion('has-in', 'Use IN with the subquery', (state, sql) => {
					// Check SQL contains IN, or filter is linked to subquery
					return sqlContains('IN')(state, sql) || hasFilterWithSubquery()(state, sql);
				}),
				criterion('has-customers', 'Query the customers table', (state, sql) => {
					return sqlContains('customers')(state, sql) || hasTable('customers')(state, sql);
				}),
				criterion('has-orders', 'Reference the orders table', (state, sql) => {
					return sqlContains('orders')(state, sql) || hasSubqueryWithTable('orders')(state, sql);
				})
			]
		},
		{
			id: 'sub-3',
			title: 'Most Expensive Product',
			description: 'Find the product(s) with the highest price using a subquery.',
			hint: 'Write: SELECT * FROM products WHERE price = (SELECT MAX(price) FROM products)\n\nOr visually: Add products table, create a WHERE subquery with MAX(price), link to price filter with = operator.',
			criteria: [
				criterion('has-subquery', 'Use a subquery', (state, sql) => {
					return sqlContains('(SELECT')(state, sql) || hasSubquery('where')(state, sql);
				}),
				criterion('has-max', 'Use MAX(price) in subquery', (state, sql) => {
					return sqlContains('MAX(')(state, sql) || hasSubqueryAggregate('MAX')(state, sql);
				}),
				criterion('has-products', 'Query the products table', (state, sql) => {
					return sqlContains('products')(state, sql) || hasTable('products')(state, sql);
				})
			]
		},
		{
			id: 'sub-4',
			title: 'Products Never Ordered',
			description: 'Find products that have never been included in any order.',
			hint: 'Write: SELECT * FROM products WHERE id NOT IN (SELECT product_id FROM order_items)\n\nOr visually: Add products table, create a WHERE subquery with order_items table selecting product_id.',
			criteria: [
				criterion('has-subquery', 'Use a subquery', (state, sql) => {
					return sqlContains('(SELECT')(state, sql) || hasSubquery('where')(state, sql);
				}),
				criterion('has-not-in', 'Use NOT IN', (state, sql) => {
					// Visual builder generates NOT IN when filter uses subquery with NOT IN operator
					return sqlContains('NOT IN')(state, sql) || hasFilterWithSubquery()(state, sql);
				}),
				criterion('has-products', 'Query the products table', (state, sql) => {
					return sqlContains('products')(state, sql) || hasTable('products')(state, sql);
				}),
				criterion('has-order-items', 'Reference order_items', (state, sql) => {
					return (
						sqlContains('order_items')(state, sql) || hasSubqueryWithTable('order_items')(state, sql)
					);
				})
			]
		},
		{
			id: 'sub-5',
			title: 'Scalar Subquery',
			description:
				'Show each product name and how much its price differs from the average price.',
			hint: 'Write: SELECT name, price - (SELECT AVG(price) FROM products) AS diff FROM products\n\nOr visually: Add products table, create a SELECT subquery with AVG(price), give it an alias.',
			criteria: [
				criterion('has-subquery', 'Use a subquery in SELECT', (state, sql) => {
					return sqlContains('(SELECT')(state, sql) || hasSubquery('select')(state, sql);
				}),
				criterion('has-avg', 'Calculate AVG(price)', (state, sql) => {
					return sqlContains('AVG(')(state, sql) || hasSubqueryAggregate('AVG')(state, sql);
				}),
				criterion('has-name', 'Select the name column', (state, sql) => {
					// Check SQL or visual state has name column selected
					if (sqlContains('name')(state, sql)) return true;
					return state.tables.some((t) => t.selectedColumns.has('name'));
				}),
				criterion('has-products', 'Query the products table', (state, sql) => {
					return sqlContains('products')(state, sql) || hasTable('products')(state, sql);
				})
			]
		}
	]
};
