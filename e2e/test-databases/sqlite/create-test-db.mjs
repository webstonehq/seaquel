import { readFileSync, unlinkSync } from 'fs';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';
import Database from 'better-sqlite3';

const __dirname = dirname(fileURLToPath(import.meta.url));
const dbPath = join(__dirname, 'test_sqlite.sqlite');
const sqlPath = join(__dirname, 'seed.sql');

// Remove existing database and WAL/SHM files for a clean start
for (const suffix of ['', '-wal', '-shm']) {
	try { unlinkSync(dbPath + suffix); } catch {}
}

const db = new Database(dbPath);

// Enable WAL mode and foreign keys
db.pragma('journal_mode = WAL');
db.pragma('foreign_keys = ON');

const sql = readFileSync(sqlPath, 'utf-8');
db.exec(sql);

// Verify
const counts = db
	.prepare(
		`
    SELECT 'categories' as tbl, COUNT(*) as cnt FROM categories
    UNION ALL SELECT 'products', COUNT(*) FROM products
    UNION ALL SELECT 'users', COUNT(*) FROM users
    UNION ALL SELECT 'orders', COUNT(*) FROM orders
    UNION ALL SELECT 'order_items', COUNT(*) FROM order_items
  `
	)
	.all();

for (const { tbl, cnt } of counts) {
	console.log(`${tbl}: ${cnt} rows`);
}

db.close();
console.log(`\nDatabase created at ${dbPath}`);
