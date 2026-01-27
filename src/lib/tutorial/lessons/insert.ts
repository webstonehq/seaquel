// src/lib/tutorial/lessons/insert.ts
import type { TutorialLesson } from '$lib/types';
import { criterion, sqlContains } from '../criteria';

export const insertLesson: TutorialLesson = {
	id: 'insert',
	title: 'INSERT Statements',
	introduction: `
INSERT adds new rows to a table. There are several forms:

**Insert with values:**
\`\`\`sql
INSERT INTO table (col1, col2)
VALUES (value1, value2)
\`\`\`

**Insert multiple rows:**
\`\`\`sql
INSERT INTO table (col1, col2)
VALUES (v1, v2), (v3, v4), (v5, v6)
\`\`\`

**Insert from a query:**
\`\`\`sql
INSERT INTO table (col1, col2)
SELECT col1, col2 FROM other_table
\`\`\`

Note: In this tutorial sandbox, INSERT statements are simulated and won't permanently modify data.
  `.trim(),
	challenges: [
		{
			id: 'insert-1',
			title: 'Insert a Category',
			description: 'Insert a new category with name "Accessories" and description "Phone accessories".',
			hint: 'Write: INSERT INTO categories (name, description) VALUES (\'Accessories\', \'Phone accessories\')',
			criteria: [
				criterion('has-insert', 'Use INSERT INTO', sqlContains('INSERT INTO')),
				criterion('has-categories', 'Insert into categories table', sqlContains('categories')),
				criterion('has-values', 'Use VALUES clause', sqlContains('VALUES'))
			]
		},
		{
			id: 'insert-2',
			title: 'Insert a Product',
			description:
				'Insert a new product: name "USB Cable", price 9.99, stock 100, category_id 1.',
			hint: 'Write: INSERT INTO products (name, price, stock, category_id) VALUES (\'USB Cable\', 9.99, 100, 1)',
			criteria: [
				criterion('has-insert', 'Use INSERT INTO', sqlContains('INSERT INTO')),
				criterion('has-products', 'Insert into products table', sqlContains('products')),
				criterion('has-values', 'Use VALUES clause', sqlContains('VALUES')),
				criterion('has-name', 'Include product name', sqlContains('USB Cable'))
			]
		},
		{
			id: 'insert-3',
			title: 'Insert Multiple Rows',
			description: 'Insert three new customers in a single statement.',
			hint: 'Write: INSERT INTO customers (name, email, country) VALUES (\'Alice\', \'alice@example.com\', \'USA\'), (\'Bob\', \'bob@example.com\', \'UK\'), (\'Carol\', \'carol@example.com\', \'Canada\')',
			criteria: [
				criterion('has-insert', 'Use INSERT INTO', sqlContains('INSERT INTO')),
				criterion('has-customers', 'Insert into customers table', sqlContains('customers')),
				criterion('has-multiple', 'Include multiple value sets (use commas)', sqlContains('), ('))
			]
		},
		{
			id: 'insert-4',
			title: 'Insert with SELECT',
			description:
				'Create a backup: insert all products from category 1 into a backup table using SELECT.',
			hint: 'Write: INSERT INTO products_backup (name, price) SELECT name, price FROM products WHERE category_id = 1',
			criteria: [
				criterion('has-insert', 'Use INSERT INTO', sqlContains('INSERT INTO')),
				criterion('has-select', 'Use SELECT for values', sqlContains('SELECT')),
				criterion('has-products', 'Select from products', sqlContains('products'))
			]
		},
		{
			id: 'insert-5',
			title: 'Insert with Default',
			description:
				'Insert a customer with just name and email, letting the database use defaults for other columns.',
			hint: 'Write: INSERT INTO customers (name, email) VALUES (\'New User\', \'new@example.com\')',
			criteria: [
				criterion('has-insert', 'Use INSERT INTO', sqlContains('INSERT INTO')),
				criterion('has-customers', 'Insert into customers table', sqlContains('customers')),
				criterion('has-values', 'Use VALUES clause', sqlContains('VALUES')),
				criterion('has-email', 'Include email value', sqlContains('@'))
			]
		}
	]
};
