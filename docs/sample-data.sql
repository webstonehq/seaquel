-- Seaquel Demo Database - SQLite
-- This script creates sample data for feature screenshots and GIFs

-- Drop tables if they exist
DROP TABLE IF EXISTS order_items;
DROP TABLE IF EXISTS orders;
DROP TABLE IF EXISTS products;
DROP TABLE IF EXISTS users;
DROP TABLE IF EXISTS categories;

-- Categories table
CREATE TABLE categories (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    description TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Products table
CREATE TABLE products (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    price DECIMAL(10, 2) NOT NULL,
    category_id INTEGER REFERENCES categories(id),
    stock_quantity INTEGER DEFAULT 0,
    description TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Users table
CREATE TABLE users (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    email TEXT UNIQUE NOT NULL,
    role TEXT DEFAULT 'customer',
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Orders table
CREATE TABLE orders (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER REFERENCES users(id),
    total DECIMAL(10, 2) NOT NULL,
    status TEXT DEFAULT 'pending',
    shipping_address TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Order items table
CREATE TABLE order_items (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    order_id INTEGER REFERENCES orders(id),
    product_id INTEGER REFERENCES products(id),
    quantity INTEGER NOT NULL,
    unit_price DECIMAL(10, 2) NOT NULL
);

-- Insert categories
INSERT INTO categories (name, description) VALUES
    ('Electronics', 'Computers, phones, and gadgets'),
    ('Clothing', 'Apparel and accessories'),
    ('Home & Garden', 'Furniture and outdoor items'),
    ('Books', 'Physical and digital books'),
    ('Sports', 'Equipment and activewear');

-- Insert products
INSERT INTO products (name, price, category_id, stock_quantity, description) VALUES
    ('Laptop Pro 15"', 1299.99, 1, 45, 'High-performance laptop with M3 chip'),
    ('Wireless Mouse', 49.99, 1, 230, 'Ergonomic bluetooth mouse'),
    ('USB-C Hub', 79.99, 1, 120, '7-in-1 USB-C adapter'),
    ('Mechanical Keyboard', 149.99, 1, 85, 'RGB mechanical keyboard'),
    ('4K Monitor 27"', 449.99, 1, 32, 'Ultra HD IPS display'),
    ('Noise-Canceling Headphones', 299.99, 1, 67, 'Premium wireless headphones'),
    ('Webcam HD', 89.99, 1, 150, '1080p webcam with microphone'),
    ('Portable SSD 1TB', 119.99, 1, 200, 'Fast external storage'),
    ('Cotton T-Shirt', 24.99, 2, 500, 'Premium cotton basics'),
    ('Denim Jeans', 69.99, 2, 180, 'Classic fit jeans'),
    ('Running Shoes', 129.99, 2, 95, 'Lightweight running shoes'),
    ('Winter Jacket', 189.99, 2, 60, 'Insulated winter coat'),
    ('Leather Belt', 39.99, 2, 220, 'Genuine leather belt'),
    ('Sunglasses', 79.99, 2, 145, 'UV protection sunglasses'),
    ('Standing Desk', 499.99, 3, 25, 'Electric height-adjustable desk'),
    ('Office Chair', 349.99, 3, 40, 'Ergonomic mesh chair'),
    ('LED Desk Lamp', 59.99, 3, 180, 'Adjustable LED lamp'),
    ('Plant Pot Set', 34.99, 3, 90, 'Ceramic plant pots (3-pack)'),
    ('Garden Tools Kit', 49.99, 3, 75, 'Essential gardening tools'),
    ('Programming Guide', 49.99, 4, 300, 'Learn to code in 30 days'),
    ('Database Design', 59.99, 4, 150, 'SQL and data modeling'),
    ('Fiction Bestseller', 16.99, 4, 400, 'Award-winning novel'),
    ('Cookbook Classics', 29.99, 4, 200, 'Traditional recipes'),
    ('Yoga Mat', 29.99, 5, 250, 'Non-slip exercise mat'),
    ('Dumbbells Set', 89.99, 5, 80, 'Adjustable weight set'),
    ('Tennis Racket', 119.99, 5, 55, 'Professional tennis racket'),
    ('Basketball', 34.99, 5, 120, 'Official size basketball'),
    ('Fitness Tracker', 149.99, 5, 175, 'Smart fitness watch');

-- Insert users
INSERT INTO users (name, email, role, created_at) VALUES
    ('Alice Johnson', 'alice@example.com', 'admin', '2024-01-15 09:30:00'),
    ('Bob Smith', 'bob@example.com', 'customer', '2024-02-20 14:45:00'),
    ('Carol Williams', 'carol@example.com', 'customer', '2024-03-10 11:20:00'),
    ('David Brown', 'david@example.com', 'customer', '2024-03-25 16:00:00'),
    ('Emma Davis', 'emma@example.com', 'customer', '2024-04-05 10:15:00'),
    ('Frank Miller', 'frank@example.com', 'customer', '2024-04-18 13:30:00'),
    ('Grace Wilson', 'grace@example.com', 'moderator', '2024-05-02 08:45:00'),
    ('Henry Taylor', 'henry@example.com', 'customer', '2024-05-15 15:20:00'),
    ('Ivy Anderson', 'ivy@example.com', 'customer', '2024-06-01 12:00:00'),
    ('Jack Thomas', 'jack@example.com', 'customer', '2024-06-20 09:10:00'),
    ('Karen Martinez', 'karen@example.com', 'customer', '2024-07-08 14:30:00'),
    ('Leo Garcia', 'leo@example.com', 'customer', '2024-07-22 11:45:00'),
    ('Mia Robinson', 'mia@example.com', 'customer', '2024-08-05 16:20:00'),
    ('Noah Clark', 'noah@example.com', 'customer', '2024-08-19 10:00:00'),
    ('Olivia Lewis', 'olivia@example.com', 'customer', '2024-09-03 13:15:00');

-- Insert orders with varied statuses and dates
INSERT INTO orders (user_id, total, status, shipping_address, created_at) VALUES
    (2, 1349.98, 'completed', '123 Main St, New York, NY', '2024-06-15 10:30:00'),
    (3, 299.99, 'completed', '456 Oak Ave, Los Angeles, CA', '2024-06-20 14:20:00'),
    (4, 549.97, 'shipped', '789 Pine Rd, Chicago, IL', '2024-07-05 09:15:00'),
    (5, 189.99, 'completed', '321 Elm St, Houston, TX', '2024-07-12 16:45:00'),
    (2, 129.98, 'completed', '123 Main St, New York, NY', '2024-07-25 11:30:00'),
    (6, 849.98, 'shipped', '654 Maple Dr, Phoenix, AZ', '2024-08-02 13:00:00'),
    (7, 449.99, 'completed', '987 Cedar Ln, Philadelphia, PA', '2024-08-10 10:20:00'),
    (8, 79.98, 'completed', '147 Birch Way, San Antonio, TX', '2024-08-18 15:40:00'),
    (9, 1599.98, 'processing', '258 Spruce Ct, San Diego, CA', '2024-08-25 08:55:00'),
    (10, 349.99, 'completed', '369 Walnut Blvd, Dallas, TX', '2024-09-01 12:10:00'),
    (3, 219.98, 'shipped', '456 Oak Ave, Los Angeles, CA', '2024-09-08 14:35:00'),
    (11, 599.98, 'processing', '741 Ash St, San Jose, CA', '2024-09-15 09:45:00'),
    (12, 149.99, 'pending', '852 Hickory Pl, Austin, TX', '2024-09-20 16:20:00'),
    (4, 89.99, 'completed', '789 Pine Rd, Chicago, IL', '2024-09-25 11:00:00'),
    (13, 1749.98, 'processing', '963 Willow Ave, Jacksonville, FL', '2024-10-01 10:30:00'),
    (14, 269.97, 'pending', '174 Poplar Dr, Fort Worth, TX', '2024-10-05 13:45:00'),
    (15, 449.99, 'pending', '285 Sycamore Rd, Columbus, OH', '2024-10-08 15:15:00'),
    (5, 59.99, 'completed', '321 Elm St, Houston, TX', '2024-10-10 09:30:00'),
    (8, 329.98, 'shipped', '147 Birch Way, San Antonio, TX', '2024-10-12 14:00:00'),
    (2, 499.99, 'processing', '123 Main St, New York, NY', '2024-10-15 11:20:00');

-- Insert order items
INSERT INTO order_items (order_id, product_id, quantity, unit_price) VALUES
    -- Order 1: Laptop + Mouse
    (1, 1, 1, 1299.99),
    (1, 2, 1, 49.99),
    -- Order 2: Headphones
    (2, 6, 1, 299.99),
    -- Order 3: Standing Desk + Chair
    (3, 15, 1, 499.99),
    (3, 2, 1, 49.99),
    -- Order 4: Winter Jacket
    (4, 12, 1, 189.99),
    -- Order 5: Running Shoes
    (5, 11, 1, 129.99),
    -- Order 6: Monitor + Keyboard
    (6, 5, 1, 449.99),
    (6, 4, 1, 149.99),
    (6, 2, 1, 49.99),
    (6, 17, 1, 59.99),
    -- Order 7: Monitor
    (7, 5, 1, 449.99),
    -- Order 8: USB-C Hub
    (8, 3, 1, 79.99),
    -- Order 9: Laptop + SSD
    (9, 1, 1, 1299.99),
    (9, 8, 1, 119.99),
    (9, 7, 1, 89.99),
    -- Order 10: Office Chair
    (10, 16, 1, 349.99),
    -- Order 11: Fitness Tracker + Yoga Mat
    (11, 28, 1, 149.99),
    (11, 24, 1, 29.99),
    (11, 25, 1, 89.99),
    -- Order 12: Standing Desk
    (12, 15, 1, 499.99),
    (12, 17, 1, 59.99),
    -- Order 13: Keyboard
    (13, 4, 1, 149.99),
    -- Order 14: Webcam
    (14, 7, 1, 89.99),
    -- Order 15: Laptop + Monitor
    (15, 1, 1, 1299.99),
    (15, 5, 1, 449.99),
    -- Order 16: Books
    (16, 20, 1, 49.99),
    (16, 21, 1, 59.99),
    (16, 22, 3, 16.99),
    (16, 23, 2, 29.99),
    -- Order 17: Monitor
    (17, 5, 1, 449.99),
    -- Order 18: LED Lamp
    (18, 17, 1, 59.99),
    -- Order 19: Headphones + Belt
    (19, 6, 1, 299.99),
    (19, 13, 1, 39.99),
    -- Order 20: Standing Desk
    (20, 15, 1, 499.99);

-- Create indexes for better query performance
CREATE INDEX idx_products_category ON products(category_id);
CREATE INDEX idx_orders_user ON orders(user_id);
CREATE INDEX idx_orders_status ON orders(status);
CREATE INDEX idx_order_items_order ON order_items(order_id);
CREATE INDEX idx_order_items_product ON order_items(product_id);

-- Verify data was inserted
SELECT 'Categories: ' || COUNT(*) FROM categories
UNION ALL
SELECT 'Products: ' || COUNT(*) FROM products
UNION ALL
SELECT 'Users: ' || COUNT(*) FROM users
UNION ALL
SELECT 'Orders: ' || COUNT(*) FROM orders
UNION ALL
SELECT 'Order Items: ' || COUNT(*) FROM order_items;
