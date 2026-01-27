// src/lib/tutorial/lessons/update.ts
import type { TutorialLesson } from '$lib/types';
import { criterion, sqlContains } from '../criteria';

export const updateLesson: TutorialLesson = {
	id: 'update',
	title: 'UPDATE Statements',
	introduction: `
UPDATE modifies existing rows in a table.

Basic syntax:
\`\`\`sql
UPDATE table
SET column1 = value1, column2 = value2
WHERE condition
\`\`\`

**IMPORTANT**: Always use a WHERE clause! Without it, UPDATE affects ALL rows in the table.

You can update:
- To a literal value: \`SET price = 99.99\`
- Based on current value: \`SET price = price * 1.1\`
- Based on another column: \`SET total = quantity * unit_price\`
- From a subquery: \`SET price = (SELECT AVG(price) FROM products)\`

Note: In this tutorial sandbox, UPDATE statements are simulated and won't permanently modify data.
  `.trim(),
	challenges: [
		{
			id: 'update-1',
			title: 'Update a Single Row',
			description: 'Update the product with id 1 to have a price of 49.99.',
			hint: 'Write: UPDATE products SET price = 49.99 WHERE id = 1',
			criteria: [
				criterion('has-update', 'Use UPDATE statement', sqlContains('UPDATE')),
				criterion('has-set', 'Use SET clause', sqlContains('SET')),
				criterion('has-where', 'Use WHERE to target specific row', sqlContains('WHERE')),
				criterion('has-products', 'Update products table', sqlContains('products'))
			]
		},
		{
			id: 'update-2',
			title: 'Update Multiple Columns',
			description: 'Update product id 2: set price to 79.99 and stock to 50.',
			hint: 'Write: UPDATE products SET price = 79.99, stock = 50 WHERE id = 2',
			criteria: [
				criterion('has-update', 'Use UPDATE statement', sqlContains('UPDATE')),
				criterion('has-price', 'Set price', sqlContains('price')),
				criterion('has-stock', 'Set stock', sqlContains('stock')),
				criterion('has-where', 'Use WHERE clause', sqlContains('WHERE'))
			]
		},
		{
			id: 'update-3',
			title: 'Percentage Increase',
			description: 'Increase all product prices by 10%.',
			hint: 'Write: UPDATE products SET price = price * 1.1',
			criteria: [
				criterion('has-update', 'Use UPDATE statement', sqlContains('UPDATE')),
				criterion('has-set', 'Use SET clause', sqlContains('SET')),
				criterion('has-calculation', 'Use price * 1.1 or similar', sqlContains('price *')),
				criterion('has-products', 'Update products table', sqlContains('products'))
			]
		},
		{
			id: 'update-4',
			title: 'Conditional Update',
			description: 'Set stock to 0 for all products where stock is less than 5.',
			hint: 'Write: UPDATE products SET stock = 0 WHERE stock < 5',
			criteria: [
				criterion('has-update', 'Use UPDATE statement', sqlContains('UPDATE')),
				criterion('has-set', 'Use SET clause', sqlContains('SET')),
				criterion('has-stock', 'Reference stock column', sqlContains('stock')),
				criterion('has-where', 'Use WHERE with condition', sqlContains('WHERE'))
			]
		},
		{
			id: 'update-5',
			title: 'Update with Subquery',
			description: 'Set all product prices in category 1 to the average price of that category.',
			hint: 'Write: UPDATE products SET price = (SELECT AVG(price) FROM products WHERE category_id = 1) WHERE category_id = 1',
			criteria: [
				criterion('has-update', 'Use UPDATE statement', sqlContains('UPDATE')),
				criterion('has-subquery', 'Use a subquery', sqlContains('(SELECT')),
				criterion('has-avg', 'Calculate AVG()', sqlContains('AVG(')),
				criterion('has-where', 'Use WHERE clause', sqlContains('WHERE'))
			]
		}
	]
};
