import { readFileSync, unlinkSync } from "fs";
import { dirname, join } from "path";
import { fileURLToPath } from "url";
import { DuckDBInstance } from "@duckdb/node-api";
import { FIXTURES } from "../shared/fixtures.mjs";

const __dirname = dirname(fileURLToPath(import.meta.url));
const dbPath = join(__dirname, "seaquel_test.duckdb");

export default async function seed() {
  // Remove existing database files for a clean start
  for (const suffix of ["", ".wal"]) {
    try {
      unlinkSync(dbPath + suffix);
    } catch {}
  }

  const instance = await DuckDBInstance.create(dbPath);
  const conn = await instance.connect();

  // Schema + all_types (DDL and dialect-specific coverage insert).
  const schema = readFileSync(join(__dirname, "schema.sql"), "utf-8");
  await conn.run(schema);

  // Bulk-load shared fixtures.
  for (const { table, columns, rows } of FIXTURES) {
    if (rows.length === 0) continue;
    const BATCH = 500;
    for (let i = 0; i < rows.length; i += BATCH) {
      const batch = rows.slice(i, i + BATCH);
      const params = [];
      const valueClauses = batch.map((row) => {
        for (const col of columns) params.push(row[col]);
        return `(${columns.map(() => "?").join(", ")})`;
      });
      await conn.run(
        `INSERT INTO ${table} (${columns.join(", ")}) VALUES ${valueClauses.join(", ")}`,
        params,
      );
    }
    console.log(`${table}: ${rows.length} rows`);
  }

  // Verify
  const reader = await conn.runAndReadAll(`
      SELECT 'categories' as tbl, COUNT(*) as cnt FROM categories
      UNION ALL SELECT 'products', COUNT(*) FROM products
      UNION ALL SELECT 'users', COUNT(*) FROM users
      UNION ALL SELECT 'orders', COUNT(*) FROM orders
      UNION ALL SELECT 'order_items', COUNT(*) FROM order_items
      UNION ALL SELECT 'all_types', COUNT(*) FROM all_types
    `);

  for (const row of reader.getRows()) {
    // oxlint-disable-next-line typescript/restrict-template-expressions
    console.log(`  ${row[0]}: ${row[1]}`);
  }

  conn.closeSync();
  console.log(`DuckDB seeded successfully (${dbPath})`);
}
