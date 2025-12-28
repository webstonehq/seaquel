/**
 * Complete Tauri IPC mock for E2E testing with Playwright.
 *
 * This module provides full mock implementations of Tauri plugins:
 * - @tauri-apps/plugin-sql: Real SQLite database via better-sqlite3
 * - @tauri-apps/plugin-store: In-memory key-value store
 * - SSH tunnel commands: Mock responses
 */

import type { Page } from '@playwright/test';
import * as fs from 'fs';
import * as path from 'path';
import { createRequire } from 'module';

const require = createRequire(import.meta.url);
const Database = require('better-sqlite3');

// Database type
type DatabaseInstance = ReturnType<typeof Database>;

// Track databases and stores
interface TestDatabaseInfo {
  db: DatabaseInstance;
  path: string;
}

const testDatabases: Map<string, TestDatabaseInfo> = new Map();
const testStores: Map<string, Record<string, unknown>> = new Map();
let dbCounter = 0;

/**
 * Create a temporary SQLite database for testing with sample data
 */
export function createTestDatabase(): { path: string; connectionString: string } {
  const tmpDir = path.join(process.cwd(), '.playwright-tmp');
  if (!fs.existsSync(tmpDir)) {
    fs.mkdirSync(tmpDir, { recursive: true });
  }

  const dbPath = path.join(tmpDir, `test-db-${Date.now()}-${dbCounter++}.sqlite`);
  const connectionString = `sqlite:${dbPath}`;

  const db = new Database(dbPath);

  // Create sample tables
  db.exec(`
    CREATE TABLE IF NOT EXISTS users (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      name TEXT NOT NULL,
      email TEXT UNIQUE,
      created_at DATETIME DEFAULT CURRENT_TIMESTAMP
    );

    CREATE TABLE IF NOT EXISTS products (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      name TEXT NOT NULL,
      price REAL NOT NULL,
      category TEXT
    );

    CREATE TABLE IF NOT EXISTS orders (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      user_id INTEGER REFERENCES users(id),
      product_id INTEGER REFERENCES products(id),
      quantity INTEGER DEFAULT 1,
      order_date DATETIME DEFAULT CURRENT_TIMESTAMP
    );

    INSERT INTO users (name, email) VALUES
      ('Alice', 'alice@example.com'),
      ('Bob', 'bob@example.com'),
      ('Charlie', 'charlie@example.com');

    INSERT INTO products (name, price, category) VALUES
      ('Widget', 9.99, 'Electronics'),
      ('Gadget', 19.99, 'Electronics'),
      ('Gizmo', 14.99, 'Tools');

    INSERT INTO orders (user_id, product_id, quantity) VALUES
      (1, 1, 2),
      (2, 2, 1),
      (1, 3, 3);
  `);

  testDatabases.set(connectionString, { db, path: dbPath });

  return { path: dbPath, connectionString };
}

/**
 * Normalize a SQLite connection string for consistent lookup
 * sqlite:///path, sqlite://path, sqlite:/path -> sqlite:/path
 */
function normalizeConnectionString(connStr: string): string {
  if (connStr.startsWith('sqlite:')) {
    // Extract the path portion
    let path = connStr.slice(7); // Remove "sqlite:"
    // Remove leading slashes but keep one for absolute paths
    while (path.startsWith('//')) {
      path = path.slice(1);
    }
    // Ensure absolute paths start with /
    if (!path.startsWith('/') && !path.startsWith('.')) {
      path = '/' + path;
    }
    return 'sqlite:' + path;
  }
  return connStr;
}

/**
 * Execute a SELECT query
 */
function executeSelect(connectionString: string, query: string, params: unknown[] = []): unknown[] {
  const normalizedConnStr = normalizeConnectionString(connectionString);
  const dbInfo = testDatabases.get(normalizedConnStr);
  if (!dbInfo) {
    // Try to find a match with normalized keys
    for (const [key, value] of testDatabases.entries()) {
      if (normalizeConnectionString(key) === normalizedConnStr) {
        return executeSelectInternal(value.db, query, params);
      }
    }
    throw new Error(`Database not found: ${connectionString} (normalized: ${normalizedConnStr})`);
  }
  return executeSelectInternal(dbInfo.db, query, params);
}

function executeSelectInternal(db: DatabaseInstance, query: string, params: unknown[]): unknown[] {
  try {
    const stmt = db.prepare(query);
    return stmt.all(...params);
  } catch (error) {
    throw new Error(`SQL Error: ${(error as Error).message}`);
  }
}

/**
 * Execute an INSERT/UPDATE/DELETE query
 */
function executeModify(connectionString: string, query: string, params: unknown[] = []): { rowsAffected: number; lastInsertId: number } {
  const normalizedConnStr = normalizeConnectionString(connectionString);
  let db: DatabaseInstance | null = null;

  const dbInfo = testDatabases.get(normalizedConnStr);
  if (dbInfo) {
    db = dbInfo.db;
  } else {
    // Try to find a match with normalized keys
    for (const [key, value] of testDatabases.entries()) {
      if (normalizeConnectionString(key) === normalizedConnStr) {
        db = value.db;
        break;
      }
    }
  }

  if (!db) {
    throw new Error(`Database not found: ${connectionString} (normalized: ${normalizedConnStr})`);
  }

  try {
    const stmt = db.prepare(query);
    const result = stmt.run(...params);
    return {
      rowsAffected: result.changes,
      lastInsertId: Number(result.lastInsertRowid),
    };
  } catch (error) {
    throw new Error(`SQL Error: ${(error as Error).message}`);
  }
}

/**
 * Get store data
 */
function getStoreValue(storePath: string, key: string): unknown {
  const store = testStores.get(storePath) || {};
  return store[key];
}

/**
 * Set store data
 */
function setStoreValue(storePath: string, key: string, value: unknown): void {
  if (!testStores.has(storePath)) {
    testStores.set(storePath, {});
  }
  testStores.get(storePath)![key] = value;
}

/**
 * Clear store data
 */
function clearStore(storePath: string): void {
  testStores.set(storePath, {});
}

/**
 * Clean up all test databases
 */
export function cleanupTestDatabases(): void {
  Array.from(testDatabases.entries()).forEach(([, { db, path: dbPath }]) => {
    try {
      db.close();
      if (fs.existsSync(dbPath)) {
        fs.unlinkSync(dbPath);
      }
    } catch {
      // Ignore cleanup errors
    }
  });
  testDatabases.clear();
  testStores.clear();

  const tmpDir = path.join(process.cwd(), '.playwright-tmp');
  if (fs.existsSync(tmpDir)) {
    try {
      fs.rmSync(tmpDir, { recursive: true });
    } catch {
      // Ignore
    }
  }
}

/**
 * Clear all stores (between tests)
 */
export function clearMockStores(): void {
  testStores.clear();
}

/**
 * Inject complete Tauri mocks into a Playwright page
 */
export async function injectTauriMocks(page: Page, testDbConnectionString: string): Promise<void> {
  // Expose Node.js functions to the browser
  await page.exposeFunction('__tauriMock_dbSelect', async (connectionString: string, query: string, params: unknown[]) => {
    return executeSelect(connectionString, query, params);
  });

  await page.exposeFunction('__tauriMock_dbExecute', async (connectionString: string, query: string, params: unknown[]) => {
    return executeModify(connectionString, query, params);
  });

  await page.exposeFunction('__tauriMock_storeGet', async (storePath: string, key: string) => {
    const result = getStoreValue(storePath, key);
    console.log(`[Node] storeGet(${storePath}, ${key}) = ${JSON.stringify(result)}`);
    return result;
  });

  await page.exposeFunction('__tauriMock_storeSet', async (storePath: string, key: string, value: unknown) => {
    console.log(`[Node] storeSet(${storePath}, ${key}, ${JSON.stringify(value)})`);
    setStoreValue(storePath, key, value);
  });

  await page.exposeFunction('__tauriMock_storeClear', async (storePath: string) => {
    clearStore(storePath);
  });

  // Inject the browser-side mock code
  await page.addInitScript(`
    (function() {
      // Track open database connections
      const openDatabases = new Map();
      let dbIdCounter = 0;

      // Track open stores
      const openStores = new Map();
      let storeIdCounter = 0;

      // Create the Tauri internals mock
      window.__TAURI_INTERNALS__ = window.__TAURI_INTERNALS__ || {};
      window.__TAURI__ = window.__TAURI__ || {};

      // Mock OS plugin internals (platform() reads from this directly, not via invoke)
      window.__TAURI_OS_PLUGIN_INTERNALS__ = {
        platform: 'macos',
        version: '14.0.0',
        family: 'unix',
        eol: '\\n',
        arch: 'aarch64',
        locale: 'en-US',
        hostname: 'test-host',
        exe_extension: '',
      };

      // Mock invoke function - this is the core IPC handler
      window.__TAURI_INTERNALS__.invoke = async function(cmd, args) {
        console.log('[Tauri Mock] invoke:', cmd, JSON.stringify(args).substring(0, 200));

        try {
          // ========== SQL Plugin ==========
          if (cmd === 'plugin:sql|load') {
            const dbPath = args.db;
            const dbId = 'db_' + (dbIdCounter++);
            openDatabases.set(dbId, dbPath);
            console.log('[Tauri Mock] Database loaded:', dbId, '->', dbPath);
            return dbId;
          }

          if (cmd === 'plugin:sql|select') {
            const dbId = args.db;
            const connectionString = openDatabases.get(dbId);
            if (!connectionString) {
              throw new Error('Database not found: ' + dbId);
            }
            const query = args.query;
            const values = args.values || [];
            console.log('[Tauri Mock] SELECT:', query.substring(0, 100));
            const result = await window.__tauriMock_dbSelect(connectionString, query, values);
            return result;
          }

          if (cmd === 'plugin:sql|execute') {
            const dbId = args.db;
            const connectionString = openDatabases.get(dbId);
            if (!connectionString) {
              throw new Error('Database not found: ' + dbId);
            }
            const query = args.query;
            const values = args.values || [];
            console.log('[Tauri Mock] EXECUTE:', query.substring(0, 100));
            const result = await window.__tauriMock_dbExecute(connectionString, query, values);
            return result;
          }

          if (cmd === 'plugin:sql|close') {
            const dbId = args.db;
            openDatabases.delete(dbId);
            console.log('[Tauri Mock] Database closed:', dbId);
            return;
          }

          // ========== Store Plugin ==========
          if (cmd === 'plugin:store|load') {
            const storePath = args.path;
            const storeId = 'store_' + (storeIdCounter++);
            openStores.set(storeId, storePath);

            // Initialize with defaults if provided
            if (args.options && args.options.defaults) {
              for (const [key, value] of Object.entries(args.options.defaults)) {
                await window.__tauriMock_storeSet(storePath, key, value);
              }
            }

            console.log('[Tauri Mock] Store loaded:', storeId, '->', storePath);
            return { rid: storeId };
          }

          if (cmd === 'plugin:store|get') {
            // rid can be an object with 'rid' property or a string
            const storeId = typeof args.rid === 'object' ? args.rid.rid : args.rid;
            const storePath = openStores.get(storeId);
            if (!storePath) {
              return undefined;
            }
            const key = args.key;
            const result = await window.__tauriMock_storeGet(storePath, key);
            // If result is undefined, return undefined (the app will handle it)
            return result;
          }

          if (cmd === 'plugin:store|set') {
            const storeId = typeof args.rid === 'object' ? args.rid.rid : args.rid;
            const storePath = openStores.get(storeId);
            if (!storePath) {
              return;
            }
            const key = args.key;
            const value = args.value;
            await window.__tauriMock_storeSet(storePath, key, value);
            return;
          }

          if (cmd === 'plugin:store|save') {
            // No-op - our store is already persisted in memory
            return;
          }

          if (cmd === 'plugin:store|clear') {
            const storeId = typeof args.rid === 'object' ? args.rid.rid : args.rid;
            const storePath = openStores.get(storeId);
            if (storePath) {
              await window.__tauriMock_storeClear(storePath);
            }
            return;
          }

          if (cmd === 'plugin:store|close') {
            const storeId = typeof args.rid === 'object' ? args.rid.rid : args.rid;
            openStores.delete(storeId);
            return;
          }

          // ========== SSH Tunnel Commands ==========
          if (cmd === 'create_ssh_tunnel') {
            console.log('[Tauri Mock] SSH tunnel created (mock)');
            return {
              tunnelId: 'mock-tunnel-' + Date.now(),
              localPort: 15432,
            };
          }

          if (cmd === 'close_ssh_tunnel') {
            console.log('[Tauri Mock] SSH tunnel closed (mock)');
            return;
          }

          // ========== Dialog Plugin ==========
          if (cmd === 'plugin:dialog|open') {
            return '/mock/selected/file.txt';
          }

          // ========== OS Plugin ==========
          if (cmd === 'plugin:os|platform') {
            console.log('[Tauri Mock] platform() returning: macos');
            return 'macos'; // Must be 'macos' not 'darwin' for isMac() to work
          }

          if (cmd === 'plugin:os|type') {
            return 'macos';
          }

          // ========== Updater Plugin ==========
          if (cmd === 'plugin:updater|check') {
            return null; // No update available
          }

          console.warn('[Tauri Mock] Unhandled command:', cmd, args);
          return null;
        } catch (error) {
          console.error('[Tauri Mock] Error in command', cmd, ':', error);
          throw error;
        }
      };

      // Mock the callback transformer
      window.__TAURI_INTERNALS__.transformCallback = function(callback, once) {
        const id = window.__TAURI_INTERNALS__.__callbackId || 0;
        window.__TAURI_INTERNALS__.__callbackId = id + 1;
        window['__TAURI_CB_' + id] = callback;
        return id;
      };

      // Mark Tauri as available
      window.__TAURI_INTERNALS__.metadata = {
        currentWindow: { label: 'main' },
        currentWebview: { label: 'main' },
      };

      console.log('[Tauri Mock] Initialized successfully');
    })();
  `);
}
