/**
 * Database schema types for tables, columns, and indexes.
 *
 * The wire types are generated from `crates/seaquel-types` (see `./generated`);
 * edit them there and run `npm run types:gen`.
 * @module types/schema
 */

import type { SchemaTable } from "./generated/SchemaTable";

export type { ForeignKeyRef } from "./generated/ForeignKeyRef";
export type { SchemaColumn } from "./generated/SchemaColumn";
export type { SchemaIndex } from "./generated/SchemaIndex";
export type { SchemaTable } from "./generated/SchemaTable";
export type { TableKind } from "./generated/TableKind";

/**
 * Represents an open schema browser tab.
 */
export interface SchemaTab {
  /** Unique tab identifier */
  id: string;
  /** The connection this tab belongs to */
  connectionId: string;
  /** The table being viewed */
  table: SchemaTable;
}
