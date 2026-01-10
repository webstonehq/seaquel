/**
 * Sample e-commerce database schema and data for the browser demo.
 */

export const SAMPLE_SCHEMA = `
-- Create schema
CREATE SCHEMA IF NOT EXISTS demo;

-- Customers table
CREATE TABLE demo.customers (
  id INTEGER PRIMARY KEY,
  email VARCHAR(255) NOT NULL,
  first_name VARCHAR(100),
  last_name VARCHAR(100),
  created_at TIMESTAMP,
  country VARCHAR(2)
);

-- Products table
CREATE TABLE demo.products (
  id INTEGER PRIMARY KEY,
  name VARCHAR(255) NOT NULL,
  category VARCHAR(100),
  price DECIMAL(10, 2),
  stock_quantity INTEGER,
  created_at TIMESTAMP
);

-- Orders table
CREATE TABLE demo.orders (
  id INTEGER PRIMARY KEY,
  customer_id INTEGER REFERENCES demo.customers(id),
  status VARCHAR(50),
  total_amount DECIMAL(10, 2),
  created_at TIMESTAMP,
  shipped_at TIMESTAMP
);

-- Order items table (junction table for orders and products)
CREATE TABLE demo.order_items (
  id INTEGER PRIMARY KEY,
  order_id INTEGER REFERENCES demo.orders(id),
  product_id INTEGER REFERENCES demo.products(id),
  quantity INTEGER,
  unit_price DECIMAL(10, 2)
);
`;

export const SAMPLE_DATA = `
-- Insert customers
INSERT INTO demo.customers VALUES (1, 'alice@example.com', 'Alice', 'Johnson', '2024-01-15 10:30:00', 'US');
INSERT INTO demo.customers VALUES (2, 'bob@example.com', 'Bob', 'Smith', '2024-01-20 14:45:00', 'US');
INSERT INTO demo.customers VALUES (3, 'carol@example.com', 'Carol', 'Williams', '2024-02-01 09:15:00', 'CA');
INSERT INTO demo.customers VALUES (4, 'david@example.com', 'David', 'Brown', '2024-02-10 16:20:00', 'UK');
INSERT INTO demo.customers VALUES (5, 'emma@example.com', 'Emma', 'Davis', '2024-02-15 11:00:00', 'DE');
INSERT INTO demo.customers VALUES (6, 'frank@example.com', 'Frank', 'Miller', '2024-02-20 13:30:00', 'FR');
INSERT INTO demo.customers VALUES (7, 'grace@example.com', 'Grace', 'Wilson', '2024-03-01 10:00:00', 'JP');
INSERT INTO demo.customers VALUES (8, 'henry@example.com', 'Henry', 'Moore', '2024-03-05 15:45:00', 'AU');
INSERT INTO demo.customers VALUES (9, 'iris@example.com', 'Iris', 'Taylor', '2024-03-10 08:30:00', 'US');
INSERT INTO demo.customers VALUES (10, 'jack@example.com', 'Jack', 'Anderson', '2024-03-15 12:00:00', 'CA');

-- Insert products
INSERT INTO demo.products VALUES (1, 'Wireless Mouse', 'Electronics', 29.99, 150, '2024-01-01 00:00:00');
INSERT INTO demo.products VALUES (2, 'Mechanical Keyboard', 'Electronics', 89.99, 75, '2024-01-01 00:00:00');
INSERT INTO demo.products VALUES (3, 'USB-C Hub', 'Electronics', 49.99, 200, '2024-01-05 00:00:00');
INSERT INTO demo.products VALUES (4, 'Monitor Stand', 'Accessories', 39.99, 100, '2024-01-10 00:00:00');
INSERT INTO demo.products VALUES (5, 'Desk Lamp', 'Accessories', 24.99, 120, '2024-01-15 00:00:00');
INSERT INTO demo.products VALUES (6, 'Webcam HD', 'Electronics', 69.99, 80, '2024-02-01 00:00:00');
INSERT INTO demo.products VALUES (7, 'Headphones', 'Electronics', 149.99, 60, '2024-02-05 00:00:00');
INSERT INTO demo.products VALUES (8, 'Mouse Pad XL', 'Accessories', 19.99, 300, '2024-02-10 00:00:00');
INSERT INTO demo.products VALUES (9, 'Cable Organizer', 'Accessories', 12.99, 250, '2024-02-15 00:00:00');
INSERT INTO demo.products VALUES (10, 'Laptop Stand', 'Accessories', 59.99, 90, '2024-02-20 00:00:00');
INSERT INTO demo.products VALUES (11, 'External SSD 1TB', 'Storage', 99.99, 45, '2024-03-01 00:00:00');
INSERT INTO demo.products VALUES (12, 'USB Flash Drive 64GB', 'Storage', 14.99, 500, '2024-03-05 00:00:00');

-- Insert orders
INSERT INTO demo.orders VALUES (1, 1, 'completed', 119.98, '2024-02-01 10:00:00', '2024-02-03 14:00:00');
INSERT INTO demo.orders VALUES (2, 2, 'completed', 89.99, '2024-02-05 11:30:00', '2024-02-07 10:00:00');
INSERT INTO demo.orders VALUES (3, 3, 'completed', 164.97, '2024-02-10 14:00:00', '2024-02-12 16:00:00');
INSERT INTO demo.orders VALUES (4, 1, 'completed', 69.99, '2024-02-15 09:00:00', '2024-02-17 11:00:00');
INSERT INTO demo.orders VALUES (5, 4, 'shipped', 249.97, '2024-03-01 16:00:00', '2024-03-03 10:00:00');
INSERT INTO demo.orders VALUES (6, 5, 'processing', 139.98, '2024-03-10 10:30:00', NULL);
INSERT INTO demo.orders VALUES (7, 6, 'processing', 79.98, '2024-03-12 14:45:00', NULL);
INSERT INTO demo.orders VALUES (8, 7, 'pending', 199.98, '2024-03-15 09:15:00', NULL);
INSERT INTO demo.orders VALUES (9, 2, 'pending', 59.99, '2024-03-16 11:00:00', NULL);
INSERT INTO demo.orders VALUES (10, 8, 'pending', 114.98, '2024-03-17 15:30:00', NULL);

-- Insert order items
INSERT INTO demo.order_items VALUES (1, 1, 1, 2, 29.99);
INSERT INTO demo.order_items VALUES (2, 1, 5, 2, 24.99);
INSERT INTO demo.order_items VALUES (3, 2, 2, 1, 89.99);
INSERT INTO demo.order_items VALUES (4, 3, 3, 1, 49.99);
INSERT INTO demo.order_items VALUES (5, 3, 6, 1, 69.99);
INSERT INTO demo.order_items VALUES (6, 3, 8, 2, 19.99);
INSERT INTO demo.order_items VALUES (7, 4, 6, 1, 69.99);
INSERT INTO demo.order_items VALUES (8, 5, 7, 1, 149.99);
INSERT INTO demo.order_items VALUES (9, 5, 11, 1, 99.99);
INSERT INTO demo.order_items VALUES (10, 6, 2, 1, 89.99);
INSERT INTO demo.order_items VALUES (11, 6, 3, 1, 49.99);
INSERT INTO demo.order_items VALUES (12, 7, 4, 2, 39.99);
INSERT INTO demo.order_items VALUES (13, 8, 7, 1, 149.99);
INSERT INTO demo.order_items VALUES (14, 8, 3, 1, 49.99);
INSERT INTO demo.order_items VALUES (15, 9, 10, 1, 59.99);
INSERT INTO demo.order_items VALUES (16, 10, 11, 1, 99.99);
INSERT INTO demo.order_items VALUES (17, 10, 12, 1, 14.99);
`;

/**
 * Example queries to show users what they can do.
 */
export const EXAMPLE_QUERIES = [
	{
		name: 'List all customers',
		query: 'SELECT * FROM demo.customers ORDER BY created_at DESC;'
	},
	{
		name: 'Products by category',
		query: `SELECT category, COUNT(*) as count, AVG(price) as avg_price
FROM demo.products
GROUP BY category
ORDER BY count DESC;`
	},
	{
		name: 'Recent orders with customer info',
		query: `SELECT o.id, c.first_name, c.last_name, o.status, o.total_amount, o.created_at
FROM demo.orders o
JOIN demo.customers c ON o.customer_id = c.id
ORDER BY o.created_at DESC
LIMIT 10;`
	},
	{
		name: 'Top selling products',
		query: `SELECT p.name, SUM(oi.quantity) as total_sold, SUM(oi.quantity * oi.unit_price) as revenue
FROM demo.order_items oi
JOIN demo.products p ON oi.product_id = p.id
GROUP BY p.id, p.name
ORDER BY total_sold DESC;`
	},
	{
		name: 'Customer order summary',
		query: `SELECT c.first_name, c.last_name, COUNT(o.id) as order_count, SUM(o.total_amount) as total_spent
FROM demo.customers c
LEFT JOIN demo.orders o ON c.id = o.customer_id
GROUP BY c.id, c.first_name, c.last_name
ORDER BY total_spent DESC;`
	}
];
