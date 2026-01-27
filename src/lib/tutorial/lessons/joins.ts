// src/lib/tutorial/lessons/joins.ts
import type { TutorialLesson } from '$lib/types';
import {
	criterion,
	hasTable,
	hasColumn,
	hasJoinOn,
	hasJoinType
} from '../criteria';

export const joinsLesson: TutorialLesson = {
	id: 'joins',
	title: 'JOINs',
	introduction: `
JOINs combine rows from two or more tables based on a related column. This is one of the most powerful features of SQL.

There are several types of JOINs:
- **INNER JOIN** - Returns only rows that have matching values in both tables
- **LEFT JOIN** - Returns all rows from the left table, plus matching rows from the right
- **RIGHT JOIN** - Returns all rows from the right table, plus matching rows from the left
- **FULL JOIN** - Returns all rows when there's a match in either table

In the query builder, drag two tables onto the canvas, then connect them by dragging from a column in one table to the matching column in another.
  `.trim(),
	challenges: [
		{
			id: 'joins-1',
			title: 'Your First JOIN',
			description:
				'Join the products table with the categories table to see which category each product belongs to.',
			hint: 'Drag both tables onto the canvas, then connect products.category_id to categories.id by clicking and dragging.',
			criteria: [
				criterion('has-products', 'Add the products table', hasTable('products')),
				criterion('has-categories', 'Add the categories table', hasTable('categories')),
				criterion(
					'has-join',
					'Join products.category_id to categories.id',
					hasJoinOn('products', 'category_id', 'categories', 'id')
				)
			]
		},
		{
			id: 'joins-2',
			title: 'Select from Both Tables',
			description:
				'Now select the product name and the category name to see them together in your results.',
			hint: 'After joining the tables, check the name column in both the products and categories tables.',
			criteria: [
				criterion('has-products', 'Add the products table', hasTable('products')),
				criterion('has-categories', 'Add the categories table', hasTable('categories')),
				criterion(
					'has-join',
					'Join products.category_id to categories.id',
					hasJoinOn('products', 'category_id', 'categories', 'id')
				),
				criterion(
					'has-product-name',
					'Select products.name',
					hasColumn('products', 'name')
				),
				criterion(
					'has-category-name',
					'Select categories.name',
					hasColumn('categories', 'name')
				)
			]
		},
		{
			id: 'joins-3',
			title: 'Orders and Customers',
			description:
				'Join orders with customers to see who placed each order. Select customer name and order total.',
			hint: 'Connect orders.customer_id to customers.id, then select the relevant columns.',
			criteria: [
				criterion('has-orders', 'Add the orders table', hasTable('orders')),
				criterion('has-customers', 'Add the customers table', hasTable('customers')),
				criterion(
					'has-join',
					'Join orders.customer_id to customers.id',
					hasJoinOn('orders', 'customer_id', 'customers', 'id')
				),
				criterion('has-name', 'Select customers.name', hasColumn('customers', 'name')),
				criterion('has-total', 'Select orders.total', hasColumn('orders', 'total'))
			]
		},
		{
			id: 'joins-4',
			title: 'LEFT JOIN',
			description:
				'Use a LEFT JOIN to show all products, including those without any reviews. Select product name and review rating.',
			hint: 'After creating the join, click on the join line and change the type to LEFT JOIN.',
			criteria: [
				criterion('has-products', 'Add the products table', hasTable('products')),
				criterion('has-reviews', 'Add the reviews table', hasTable('reviews')),
				criterion(
					'has-join',
					'Join products.id to reviews.product_id',
					hasJoinOn('products', 'id', 'reviews', 'product_id')
				),
				criterion('has-left-join', 'Use a LEFT JOIN', hasJoinType('LEFT')),
				criterion('has-name', 'Select products.name', hasColumn('products', 'name')),
				criterion('has-rating', 'Select reviews.rating', hasColumn('reviews', 'rating'))
			]
		},
		{
			id: 'joins-5',
			title: 'Three-Table JOIN',
			description:
				'Join order_items with products and orders to see a detailed order breakdown. Show order ID, product name, and quantity.',
			hint: 'Add all three tables and connect order_items to both orders (via order_id) and products (via product_id).',
			criteria: [
				criterion('has-order-items', 'Add the order_items table', hasTable('order_items')),
				criterion('has-products', 'Add the products table', hasTable('products')),
				criterion('has-orders', 'Add the orders table', hasTable('orders')),
				criterion(
					'has-join-products',
					'Join order_items.product_id to products.id',
					hasJoinOn('order_items', 'product_id', 'products', 'id')
				),
				criterion(
					'has-join-orders',
					'Join order_items.order_id to orders.id',
					hasJoinOn('order_items', 'order_id', 'orders', 'id')
				),
				criterion('has-order-id', 'Select orders.id', hasColumn('orders', 'id')),
				criterion('has-product-name', 'Select products.name', hasColumn('products', 'name')),
				criterion('has-quantity', 'Select order_items.quantity', hasColumn('order_items', 'quantity'))
			]
		}
	]
};
