// src/lib/tutorial/schema.ts
import type { TutorialTable } from '$lib/types';

export const TUTORIAL_SCHEMA: TutorialTable[] = [
  {
    name: 'categories',
    columns: [
      { name: 'id', type: 'INTEGER', primaryKey: true },
      { name: 'name', type: 'TEXT' },
      { name: 'description', type: 'TEXT' },
    ],
  },
  {
    name: 'products',
    columns: [
      { name: 'id', type: 'INTEGER', primaryKey: true },
      { name: 'name', type: 'TEXT' },
      { name: 'description', type: 'TEXT' },
      { name: 'price', type: 'DECIMAL' },
      { name: 'stock', type: 'INTEGER' },
      { name: 'category_id', type: 'INTEGER', foreignKey: { table: 'categories', column: 'id' } },
      { name: 'created_at', type: 'DATETIME' },
    ],
  },
  {
    name: 'customers',
    columns: [
      { name: 'id', type: 'INTEGER', primaryKey: true },
      { name: 'name', type: 'TEXT' },
      { name: 'email', type: 'TEXT' },
      { name: 'country', type: 'TEXT' },
      { name: 'created_at', type: 'DATETIME' },
    ],
  },
  {
    name: 'orders',
    columns: [
      { name: 'id', type: 'INTEGER', primaryKey: true },
      { name: 'customer_id', type: 'INTEGER', foreignKey: { table: 'customers', column: 'id' } },
      { name: 'status', type: 'TEXT' },
      { name: 'total', type: 'DECIMAL' },
      { name: 'created_at', type: 'DATETIME' },
    ],
  },
  {
    name: 'order_items',
    columns: [
      { name: 'id', type: 'INTEGER', primaryKey: true },
      { name: 'order_id', type: 'INTEGER', foreignKey: { table: 'orders', column: 'id' } },
      { name: 'product_id', type: 'INTEGER', foreignKey: { table: 'products', column: 'id' } },
      { name: 'quantity', type: 'INTEGER' },
      { name: 'unit_price', type: 'DECIMAL' },
    ],
  },
  {
    name: 'reviews',
    columns: [
      { name: 'id', type: 'INTEGER', primaryKey: true },
      { name: 'product_id', type: 'INTEGER', foreignKey: { table: 'products', column: 'id' } },
      { name: 'customer_id', type: 'INTEGER', foreignKey: { table: 'customers', column: 'id' } },
      { name: 'rating', type: 'INTEGER' },
      { name: 'comment', type: 'TEXT' },
      { name: 'created_at', type: 'DATETIME' },
    ],
  },
];

/** Get a table by name */
export function getTable(name: string): TutorialTable | undefined {
  return TUTORIAL_SCHEMA.find((t) => t.name === name);
}

/** Get all table names */
export function getTableNames(): string[] {
  return TUTORIAL_SCHEMA.map((t) => t.name);
}
