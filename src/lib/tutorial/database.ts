// src/lib/tutorial/database.ts
import Database from '@tauri-apps/plugin-sql';

let tutorialDb: Database | null = null;

/**
 * Get or create the tutorial SQLite database connection.
 * The database is created in-memory and seeded on first access.
 */
export async function getTutorialDatabase(): Promise<Database> {
  if (tutorialDb) {
    return tutorialDb;
  }

  // Create in-memory SQLite database
  tutorialDb = await Database.load('sqlite::memory:');

  // Seed the database
  await seedDatabase(tutorialDb);

  return tutorialDb;
}

async function seedDatabase(db: Database): Promise<void> {
  // Create tables
  await db.execute(`
    CREATE TABLE IF NOT EXISTS categories (
      id INTEGER PRIMARY KEY,
      name TEXT NOT NULL,
      description TEXT
    )
  `);

  await db.execute(`
    CREATE TABLE IF NOT EXISTS products (
      id INTEGER PRIMARY KEY,
      name TEXT NOT NULL,
      description TEXT,
      price DECIMAL(10,2) NOT NULL,
      stock INTEGER DEFAULT 0,
      category_id INTEGER REFERENCES categories(id),
      created_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )
  `);

  await db.execute(`
    CREATE TABLE IF NOT EXISTS customers (
      id INTEGER PRIMARY KEY,
      name TEXT NOT NULL,
      email TEXT UNIQUE NOT NULL,
      country TEXT,
      created_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )
  `);

  await db.execute(`
    CREATE TABLE IF NOT EXISTS orders (
      id INTEGER PRIMARY KEY,
      customer_id INTEGER REFERENCES customers(id),
      status TEXT CHECK(status IN ('pending', 'shipped', 'delivered', 'cancelled')) DEFAULT 'pending',
      total DECIMAL(10,2),
      created_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )
  `);

  await db.execute(`
    CREATE TABLE IF NOT EXISTS order_items (
      id INTEGER PRIMARY KEY,
      order_id INTEGER REFERENCES orders(id),
      product_id INTEGER REFERENCES products(id),
      quantity INTEGER NOT NULL,
      unit_price DECIMAL(10,2) NOT NULL
    )
  `);

  await db.execute(`
    CREATE TABLE IF NOT EXISTS reviews (
      id INTEGER PRIMARY KEY,
      product_id INTEGER REFERENCES products(id),
      customer_id INTEGER REFERENCES customers(id),
      rating INTEGER CHECK(rating >= 1 AND rating <= 5),
      comment TEXT,
      created_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )
  `);

  // Seed categories
  await db.execute(`
    INSERT INTO categories (id, name, description) VALUES
      (1, 'Electronics', 'Phones, laptops, and gadgets'),
      (2, 'Clothing', 'Apparel and accessories'),
      (3, 'Books', 'Fiction and non-fiction'),
      (4, 'Home', 'Furniture and decor'),
      (5, 'Sports', 'Equipment and gear')
  `);

  // Seed sample products (10 products)
  await db.execute(`
    INSERT INTO products (id, name, description, price, stock, category_id) VALUES
      (1, 'Smartphone X', 'Latest smartphone with 5G', 799.99, 50, 1),
      (2, 'Laptop Pro', '15-inch professional laptop', 1299.99, 30, 1),
      (3, 'Wireless Earbuds', 'Noise-canceling earbuds', 149.99, 100, 1),
      (4, 'Cotton T-Shirt', 'Classic fit cotton tee', 24.99, 200, 2),
      (5, 'Denim Jeans', 'Slim fit blue jeans', 59.99, 100, 2),
      (6, 'The Great Novel', 'Bestselling fiction', 14.99, 200, 3),
      (7, 'Learn SQL', 'Database fundamentals', 39.99, 100, 3),
      (8, 'Coffee Table', 'Modern wooden table', 199.99, 20, 4),
      (9, 'Yoga Mat', 'Non-slip exercise mat', 29.99, 100, 5),
      (10, 'Dumbbells', '10lb dumbbell pair', 49.99, 50, 5)
  `);

  // Seed sample customers
  await db.execute(`
    INSERT INTO customers (id, name, email, country) VALUES
      (1, 'Alice Johnson', 'alice@email.com', 'USA'),
      (2, 'Bob Smith', 'bob@email.com', 'USA'),
      (3, 'Carol White', 'carol@email.com', 'Canada'),
      (4, 'David Brown', 'david@email.com', 'UK'),
      (5, 'Emma Davis', 'emma@email.com', 'Australia')
  `);

  // Seed sample orders
  await db.execute(`
    INSERT INTO orders (id, customer_id, status, total) VALUES
      (1, 1, 'delivered', 849.98),
      (2, 1, 'shipped', 24.99),
      (3, 2, 'pending', 1349.98),
      (4, 3, 'delivered', 89.98),
      (5, 4, 'cancelled', 199.99)
  `);

  // Seed sample order items
  await db.execute(`
    INSERT INTO order_items (id, order_id, product_id, quantity, unit_price) VALUES
      (1, 1, 1, 1, 799.99),
      (2, 1, 3, 1, 49.99),
      (3, 2, 4, 1, 24.99),
      (4, 3, 2, 1, 1299.99),
      (5, 3, 10, 1, 49.99),
      (6, 4, 5, 1, 59.99),
      (7, 4, 9, 1, 29.99),
      (8, 5, 8, 1, 199.99)
  `);

  // Seed sample reviews
  await db.execute(`
    INSERT INTO reviews (id, product_id, customer_id, rating, comment) VALUES
      (1, 1, 1, 5, 'Excellent phone!'),
      (2, 2, 2, 4, 'Great laptop, a bit pricey'),
      (3, 4, 3, 5, 'Very comfortable'),
      (4, 6, 4, 3, 'Good read'),
      (5, 9, 5, 5, 'Perfect for yoga')
  `);
}

/**
 * Execute a query on the tutorial database and return results.
 */
export async function executeQuery(sql: string): Promise<Record<string, unknown>[]> {
  const db = await getTutorialDatabase();
  return db.select<Record<string, unknown>[]>(sql);
}

/**
 * Close the tutorial database connection.
 */
export async function closeTutorialDatabase(): Promise<void> {
  if (tutorialDb) {
    await tutorialDb.close();
    tutorialDb = null;
  }
}
