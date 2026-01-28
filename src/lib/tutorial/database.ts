// src/lib/tutorial/database.ts
import type { SchemaTable } from '$lib/types';
import { getDuckDBProvider, type DatabaseProvider } from '$lib/providers';
import { isTauri } from '$lib/utils/environment';

let tutorialProvider: DatabaseProvider | null = null;
let tutorialConnectionId: string | null = null;

// For Tauri mode, we use the Tauri SQL plugin directly for SQLite
type TauriDatabase = {
  execute: (sql: string, params?: unknown[]) => Promise<unknown>;
  select: <T>(sql: string, params?: unknown[]) => Promise<T>;
  close: () => Promise<boolean>;
};
let tauriDb: TauriDatabase | null = null;

/**
 * Get the static schema for the tutorial database.
 * Used by MonacoEditor for autocomplete.
 */
export function getTutorialSchema(): SchemaTable[] {
  return [
    {
      name: 'categories',
      schema: 'main',
      type: 'table',
      columns: [
        { name: 'id', type: 'INTEGER', nullable: false, isPrimaryKey: true, isForeignKey: false },
        { name: 'name', type: 'TEXT', nullable: false, isPrimaryKey: false, isForeignKey: false },
        { name: 'description', type: 'TEXT', nullable: true, isPrimaryKey: false, isForeignKey: false }
      ],
      indexes: []
    },
    {
      name: 'products',
      schema: 'main',
      type: 'table',
      columns: [
        { name: 'id', type: 'INTEGER', nullable: false, isPrimaryKey: true, isForeignKey: false },
        { name: 'name', type: 'TEXT', nullable: false, isPrimaryKey: false, isForeignKey: false },
        { name: 'description', type: 'TEXT', nullable: true, isPrimaryKey: false, isForeignKey: false },
        { name: 'price', type: 'DECIMAL(10,2)', nullable: false, isPrimaryKey: false, isForeignKey: false },
        { name: 'stock', type: 'INTEGER', nullable: true, defaultValue: '0', isPrimaryKey: false, isForeignKey: false },
        { name: 'category_id', type: 'INTEGER', nullable: true, isPrimaryKey: false, isForeignKey: true, foreignKeyRef: { referencedSchema: 'main', referencedTable: 'categories', referencedColumn: 'id' } },
        { name: 'created_at', type: 'DATETIME', nullable: true, defaultValue: 'CURRENT_TIMESTAMP', isPrimaryKey: false, isForeignKey: false }
      ],
      indexes: []
    },
    {
      name: 'customers',
      schema: 'main',
      type: 'table',
      columns: [
        { name: 'id', type: 'INTEGER', nullable: false, isPrimaryKey: true, isForeignKey: false },
        { name: 'name', type: 'TEXT', nullable: false, isPrimaryKey: false, isForeignKey: false },
        { name: 'email', type: 'TEXT', nullable: false, isPrimaryKey: false, isForeignKey: false },
        { name: 'country', type: 'TEXT', nullable: true, isPrimaryKey: false, isForeignKey: false },
        { name: 'created_at', type: 'DATETIME', nullable: true, defaultValue: 'CURRENT_TIMESTAMP', isPrimaryKey: false, isForeignKey: false }
      ],
      indexes: []
    },
    {
      name: 'orders',
      schema: 'main',
      type: 'table',
      columns: [
        { name: 'id', type: 'INTEGER', nullable: false, isPrimaryKey: true, isForeignKey: false },
        { name: 'customer_id', type: 'INTEGER', nullable: true, isPrimaryKey: false, isForeignKey: true, foreignKeyRef: { referencedSchema: 'main', referencedTable: 'customers', referencedColumn: 'id' } },
        { name: 'status', type: 'TEXT', nullable: true, defaultValue: "'pending'", isPrimaryKey: false, isForeignKey: false },
        { name: 'total', type: 'DECIMAL(10,2)', nullable: true, isPrimaryKey: false, isForeignKey: false },
        { name: 'created_at', type: 'DATETIME', nullable: true, defaultValue: 'CURRENT_TIMESTAMP', isPrimaryKey: false, isForeignKey: false }
      ],
      indexes: []
    },
    {
      name: 'order_items',
      schema: 'main',
      type: 'table',
      columns: [
        { name: 'id', type: 'INTEGER', nullable: false, isPrimaryKey: true, isForeignKey: false },
        { name: 'order_id', type: 'INTEGER', nullable: true, isPrimaryKey: false, isForeignKey: true, foreignKeyRef: { referencedSchema: 'main', referencedTable: 'orders', referencedColumn: 'id' } },
        { name: 'product_id', type: 'INTEGER', nullable: true, isPrimaryKey: false, isForeignKey: true, foreignKeyRef: { referencedSchema: 'main', referencedTable: 'products', referencedColumn: 'id' } },
        { name: 'quantity', type: 'INTEGER', nullable: false, isPrimaryKey: false, isForeignKey: false },
        { name: 'unit_price', type: 'DECIMAL(10,2)', nullable: false, isPrimaryKey: false, isForeignKey: false }
      ],
      indexes: []
    },
    {
      name: 'reviews',
      schema: 'main',
      type: 'table',
      columns: [
        { name: 'id', type: 'INTEGER', nullable: false, isPrimaryKey: true, isForeignKey: false },
        { name: 'product_id', type: 'INTEGER', nullable: true, isPrimaryKey: false, isForeignKey: true, foreignKeyRef: { referencedSchema: 'main', referencedTable: 'products', referencedColumn: 'id' } },
        { name: 'customer_id', type: 'INTEGER', nullable: true, isPrimaryKey: false, isForeignKey: true, foreignKeyRef: { referencedSchema: 'main', referencedTable: 'customers', referencedColumn: 'id' } },
        { name: 'rating', type: 'INTEGER', nullable: true, isPrimaryKey: false, isForeignKey: false },
        { name: 'comment', type: 'TEXT', nullable: true, isPrimaryKey: false, isForeignKey: false },
        { name: 'created_at', type: 'DATETIME', nullable: true, defaultValue: 'CURRENT_TIMESTAMP', isPrimaryKey: false, isForeignKey: false }
      ],
      indexes: []
    }
  ];
}

/**
 * Get or create the tutorial database connection.
 * Uses DuckDB-WASM in browser mode, SQLite via Tauri in desktop mode.
 * The database is created in-memory and seeded on first access.
 */
async function initializeTutorialDatabase(): Promise<void> {
  if (tutorialConnectionId || tauriDb) {
    return;
  }

  if (isTauri()) {
    // In Tauri mode, use SQLite via the Tauri SQL plugin
    const Database = (await import('@tauri-apps/plugin-sql')).default;
    const db = await Database.load('sqlite::memory:');
    tauriDb = db;
    await seedDatabaseTauri(db);
  } else {
    // In browser mode, use DuckDB-WASM
    tutorialProvider = await getDuckDBProvider();
    tutorialConnectionId = await tutorialProvider.connect({
      type: 'duckdb',
      databaseName: ':memory:'
    });
    await seedDatabaseProvider(tutorialProvider, tutorialConnectionId);
  }
}

/**
 * Seed the database using Tauri SQL plugin (SQLite).
 */
async function seedDatabaseTauri(db: TauriDatabase): Promise<void> {
  await seedDatabaseWithExecutor((sql) => db.execute(sql));
}

/**
 * Seed the database using the provider system (DuckDB).
 */
async function seedDatabaseProvider(provider: DatabaseProvider, connectionId: string): Promise<void> {
  await seedDatabaseWithExecutor((sql) => provider.execute(connectionId, sql));
}

/**
 * Common seeding logic that works with any executor function.
 * Note: DuckDB uses INSERT OR REPLACE instead of INSERT OR IGNORE,
 * so we use INSERT OR REPLACE which works for both SQLite and DuckDB.
 */
async function seedDatabaseWithExecutor(execute: (sql: string) => Promise<unknown>): Promise<void> {
  // Create tables
  await execute(`
    CREATE TABLE IF NOT EXISTS categories (
      id INTEGER PRIMARY KEY,
      name TEXT NOT NULL,
      description TEXT
    )
  `);

  await execute(`
    CREATE TABLE IF NOT EXISTS products (
      id INTEGER PRIMARY KEY,
      name TEXT NOT NULL,
      description TEXT,
      price DECIMAL(10,2) NOT NULL,
      stock INTEGER DEFAULT 0,
      category_id INTEGER,
      created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
    )
  `);

  await execute(`
    CREATE TABLE IF NOT EXISTS customers (
      id INTEGER PRIMARY KEY,
      name TEXT NOT NULL,
      email TEXT NOT NULL,
      country TEXT,
      created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
    )
  `);

  await execute(`
    CREATE TABLE IF NOT EXISTS orders (
      id INTEGER PRIMARY KEY,
      customer_id INTEGER,
      status TEXT DEFAULT 'pending',
      total DECIMAL(10,2),
      created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
    )
  `);

  await execute(`
    CREATE TABLE IF NOT EXISTS order_items (
      id INTEGER PRIMARY KEY,
      order_id INTEGER,
      product_id INTEGER,
      quantity INTEGER NOT NULL,
      unit_price DECIMAL(10,2) NOT NULL
    )
  `);

  await execute(`
    CREATE TABLE IF NOT EXISTS reviews (
      id INTEGER PRIMARY KEY,
      product_id INTEGER,
      customer_id INTEGER,
      rating INTEGER,
      comment TEXT,
      created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
    )
  `);

  // Seed categories - use individual inserts to work with both SQLite and DuckDB
  await execute(`INSERT INTO categories (id, name, description) VALUES (1, 'Electronics', 'Phones, laptops, and gadgets')`);
  await execute(`INSERT INTO categories (id, name, description) VALUES (2, 'Clothing', 'Apparel and accessories')`);
  await execute(`INSERT INTO categories (id, name, description) VALUES (3, 'Books', 'Fiction and non-fiction')`);
  await execute(`INSERT INTO categories (id, name, description) VALUES (4, 'Home', 'Furniture and decor')`);
  await execute(`INSERT INTO categories (id, name, description) VALUES (5, 'Sports', 'Equipment and gear')`);

  // Seed sample products (10 products)
  await execute(`INSERT INTO products (id, name, description, price, stock, category_id) VALUES (1, 'Smartphone X', 'Latest smartphone with 5G', 799.99, 50, 1)`);
  await execute(`INSERT INTO products (id, name, description, price, stock, category_id) VALUES (2, 'Laptop Pro', '15-inch professional laptop', 1299.99, 30, 1)`);
  await execute(`INSERT INTO products (id, name, description, price, stock, category_id) VALUES (3, 'Wireless Earbuds', 'Noise-canceling earbuds', 149.99, 100, 1)`);
  await execute(`INSERT INTO products (id, name, description, price, stock, category_id) VALUES (4, 'Cotton T-Shirt', 'Classic fit cotton tee', 24.99, 200, 2)`);
  await execute(`INSERT INTO products (id, name, description, price, stock, category_id) VALUES (5, 'Denim Jeans', 'Slim fit blue jeans', 59.99, 100, 2)`);
  await execute(`INSERT INTO products (id, name, description, price, stock, category_id) VALUES (6, 'The Great Novel', 'Bestselling fiction', 14.99, 200, 3)`);
  await execute(`INSERT INTO products (id, name, description, price, stock, category_id) VALUES (7, 'Learn SQL', 'Database fundamentals', 39.99, 100, 3)`);
  await execute(`INSERT INTO products (id, name, description, price, stock, category_id) VALUES (8, 'Coffee Table', 'Modern wooden table', 199.99, 20, 4)`);
  await execute(`INSERT INTO products (id, name, description, price, stock, category_id) VALUES (9, 'Yoga Mat', 'Non-slip exercise mat', 29.99, 100, 5)`);
  await execute(`INSERT INTO products (id, name, description, price, stock, category_id) VALUES (10, 'Dumbbells', '10lb dumbbell pair', 49.99, 50, 5)`);

  // Seed sample customers
  await execute(`INSERT INTO customers (id, name, email, country) VALUES (1, 'Alice Johnson', 'alice@email.com', 'USA')`);
  await execute(`INSERT INTO customers (id, name, email, country) VALUES (2, 'Bob Smith', 'bob@email.com', 'USA')`);
  await execute(`INSERT INTO customers (id, name, email, country) VALUES (3, 'Carol White', 'carol@email.com', 'Canada')`);
  await execute(`INSERT INTO customers (id, name, email, country) VALUES (4, 'David Brown', 'david@email.com', 'UK')`);
  await execute(`INSERT INTO customers (id, name, email, country) VALUES (5, 'Emma Davis', 'emma@email.com', 'Australia')`);

  // Seed sample orders
  await execute(`INSERT INTO orders (id, customer_id, status, total) VALUES (1, 1, 'delivered', 849.98)`);
  await execute(`INSERT INTO orders (id, customer_id, status, total) VALUES (2, 1, 'shipped', 24.99)`);
  await execute(`INSERT INTO orders (id, customer_id, status, total) VALUES (3, 2, 'pending', 1349.98)`);
  await execute(`INSERT INTO orders (id, customer_id, status, total) VALUES (4, 3, 'delivered', 89.98)`);
  await execute(`INSERT INTO orders (id, customer_id, status, total) VALUES (5, 4, 'cancelled', 199.99)`);

  // Seed sample order items
  await execute(`INSERT INTO order_items (id, order_id, product_id, quantity, unit_price) VALUES (1, 1, 1, 1, 799.99)`);
  await execute(`INSERT INTO order_items (id, order_id, product_id, quantity, unit_price) VALUES (2, 1, 3, 1, 49.99)`);
  await execute(`INSERT INTO order_items (id, order_id, product_id, quantity, unit_price) VALUES (3, 2, 4, 1, 24.99)`);
  await execute(`INSERT INTO order_items (id, order_id, product_id, quantity, unit_price) VALUES (4, 3, 2, 1, 1299.99)`);
  await execute(`INSERT INTO order_items (id, order_id, product_id, quantity, unit_price) VALUES (5, 3, 10, 1, 49.99)`);
  await execute(`INSERT INTO order_items (id, order_id, product_id, quantity, unit_price) VALUES (6, 4, 5, 1, 59.99)`);
  await execute(`INSERT INTO order_items (id, order_id, product_id, quantity, unit_price) VALUES (7, 4, 9, 1, 29.99)`);
  await execute(`INSERT INTO order_items (id, order_id, product_id, quantity, unit_price) VALUES (8, 5, 8, 1, 199.99)`);

  // Seed sample reviews
  await execute(`INSERT INTO reviews (id, product_id, customer_id, rating, comment) VALUES (1, 1, 1, 5, 'Excellent phone!')`);
  await execute(`INSERT INTO reviews (id, product_id, customer_id, rating, comment) VALUES (2, 2, 2, 4, 'Great laptop, a bit pricey')`);
  await execute(`INSERT INTO reviews (id, product_id, customer_id, rating, comment) VALUES (3, 4, 3, 5, 'Very comfortable')`);
  await execute(`INSERT INTO reviews (id, product_id, customer_id, rating, comment) VALUES (4, 6, 4, 3, 'Good read')`);
  await execute(`INSERT INTO reviews (id, product_id, customer_id, rating, comment) VALUES (5, 9, 5, 5, 'Perfect for yoga')`);
}

/**
 * Execute a query on the tutorial database and return results.
 */
export async function executeQuery(sql: string): Promise<Record<string, unknown>[]> {
  await initializeTutorialDatabase();

  if (isTauri() && tauriDb) {
    return tauriDb.select<Record<string, unknown>[]>(sql);
  } else if (tutorialProvider && tutorialConnectionId) {
    return tutorialProvider.select<Record<string, unknown>>(tutorialConnectionId, sql);
  }

  throw new Error('Tutorial database not initialized');
}

/**
 * Close the tutorial database connection.
 */
export async function closeTutorialDatabase(): Promise<void> {
  if (tauriDb) {
    await tauriDb.close();
    tauriDb = null;
  }

  if (tutorialProvider && tutorialConnectionId) {
    await tutorialProvider.disconnect(tutorialConnectionId);
    tutorialProvider = null;
    tutorialConnectionId = null;
  }
}
