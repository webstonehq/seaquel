// src/lib/tutorial/lessons/aggregates.ts
import type { TutorialLesson } from '$lib/types';
import { criterion, hasTable, hasAnyColumns, sqlContains } from '../criteria';

export const aggregatesLesson: TutorialLesson = {
	id: 'aggregates',
	title: 'Aggregate Functions',
	introduction: `
Aggregate functions perform calculations on sets of rows and return a single result.

Common aggregate functions:
- **COUNT(*)** - Counts the number of rows
- **COUNT(column)** - Counts non-NULL values in a column
- **SUM(column)** - Adds up all values
- **AVG(column)** - Calculates the average
- **MIN(column)** - Finds the smallest value
- **MAX(column)** - Finds the largest value

For these challenges, you'll need to edit the SQL directly in the SQL editor panel. The visual builder will show the base query, but you'll add the aggregate functions by typing.
  `.trim(),
	challenges: [
		{
			id: 'agg-1',
			title: 'Count All Rows',
			description:
				'Count the total number of products. Use COUNT(*) to count all rows in the products table.',
			hint: 'In the SQL editor, write: SELECT COUNT(*) FROM products',
			criteria: [
				criterion('has-count', 'Use COUNT(*) in your query', sqlContains('COUNT(*)')),
				criterion('has-products', 'Query the products table', sqlContains('products'))
			]
		},
		{
			id: 'agg-2',
			title: 'Sum of Values',
			description: 'Calculate the total value of all product stock. Use SUM(price * stock).',
			hint: 'Write: SELECT SUM(price * stock) FROM products',
			criteria: [
				criterion('has-sum', 'Use SUM() in your query', sqlContains('SUM(')),
				criterion('has-products', 'Query the products table', sqlContains('products'))
			]
		},
		{
			id: 'agg-3',
			title: 'Average Price',
			description: 'Find the average price of all products using the AVG function.',
			hint: 'Write: SELECT AVG(price) FROM products',
			criteria: [
				criterion('has-avg', 'Use AVG() in your query', sqlContains('AVG(')),
				criterion('has-price', 'Calculate average of price', sqlContains('price')),
				criterion('has-products', 'Query the products table', sqlContains('products'))
			]
		},
		{
			id: 'agg-4',
			title: 'Min and Max',
			description: 'Find both the cheapest and most expensive product prices in a single query.',
			hint: 'Write: SELECT MIN(price), MAX(price) FROM products',
			criteria: [
				criterion('has-min', 'Use MIN() in your query', sqlContains('MIN(')),
				criterion('has-max', 'Use MAX() in your query', sqlContains('MAX(')),
				criterion('has-products', 'Query the products table', sqlContains('products'))
			]
		},
		{
			id: 'agg-5',
			title: 'Multiple Aggregates',
			description:
				'Create a summary of products: count them, find the average price, and the total stock.',
			hint: 'Write: SELECT COUNT(*), AVG(price), SUM(stock) FROM products',
			criteria: [
				criterion('has-count', 'Use COUNT() in your query', sqlContains('COUNT(')),
				criterion('has-avg', 'Use AVG() in your query', sqlContains('AVG(')),
				criterion('has-sum', 'Use SUM() in your query', sqlContains('SUM(')),
				criterion('has-products', 'Query the products table', sqlContains('products'))
			]
		}
	]
};
