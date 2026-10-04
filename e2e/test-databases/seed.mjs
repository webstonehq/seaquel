/**
 * E2E test database seeder.
 *
 * Dispatches to the db-specific seed script in each matching sub-directory.
 * Multiple databases are seeded in parallel, and every log line is prefixed
 * with the owning db (via AsyncLocalStorage + patched stdout/stderr).
 *
 * Usage:
 *   node e2e/test-databases/seed.mjs <database> [<database> ...]
 *   node e2e/test-databases/seed.mjs all
 *
 * Databases: postgresql, mysql, mariadb, sqlserver, sqlite, duckdb
 */

import { AsyncLocalStorage } from "async_hooks";
import { dirname, join } from "path";
import { fileURLToPath, pathToFileURL } from "url";
import { existsSync } from "fs";

const __dirname = dirname(fileURLToPath(import.meta.url));

const DATABASES = ["postgresql", "mysql", "mariadb", "sqlserver", "sqlite", "duckdb"];

const args = process.argv.slice(2);

if (args.length === 0) {
  console.error(`Usage: node ${process.argv[1]} <${DATABASES.join("|")}|all> [...]`);
  process.exit(1);
}

const targets = args.includes("all") ? DATABASES : [...new Set(args)];

for (const db of targets) {
  if (!DATABASES.includes(db)) {
    console.error(`Unknown database: ${db}`);
    console.error(`Available: ${DATABASES.join(", ")}, all`);
    process.exit(1);
  }

  const scriptPath = join(__dirname, db, "seed.mjs");
  if (!existsSync(scriptPath)) {
    console.error(`Missing seed script: ${scriptPath}`);
    process.exit(1);
  }
}

// --- Per-db log prefixing via AsyncLocalStorage ---
// Each parallel seed() runs inside als.run(db, ...). Stdout/stderr writes
// executed within that scope are prefixed with `[db] ` on each line.
/** @type {AsyncLocalStorage<string>} */
const als = new AsyncLocalStorage();

function patchStream(stream) {
  const orig = stream.write.bind(stream);
  stream.write = (chunk, ...rest) => {
    const prefix = als.getStore();
    if (!prefix) return orig(chunk, ...rest);
    const str = typeof chunk === "string" ? chunk : chunk.toString();
    // Prefix every non-empty line start; (?!$) skips trailing-newline empty lines
    const prefixed = str.replace(/^(?!$)/gm, `[${prefix}] `);
    return orig(prefixed, ...rest);
  };
}

patchStream(process.stdout);
patchStream(process.stderr);

console.log(`Seeding in parallel: ${targets.join(", ")}\n`);

const results = await Promise.allSettled(
  targets.map((db) =>
    als.run(db, async () => {
      const scriptPath = join(__dirname, db, "seed.mjs");
      // A file URL: Node's ESM loader refuses a bare Windows path (`D:\…`).
      const { default: seed } = await import(pathToFileURL(scriptPath).href);
      console.log(`starting`);
      await seed();
      console.log(`done`);
    }),
  ),
);

console.log("\n=== Summary ===");
const failures = [];
for (const [i, result] of results.entries()) {
  const db = targets[i];
  if (result.status === "fulfilled") {
    console.log(`  ${db}: ok`);
  } else {
    console.log(`  ${db}: failed`);
    failures.push({ db, reason: result.reason });
  }
}

if (failures.length > 0) {
  console.error("\nFailures:");
  for (const { db, reason } of failures) {
    console.error(`  [${db}]`, reason);
  }
  process.exit(1);
}
