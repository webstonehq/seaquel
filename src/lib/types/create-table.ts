/**
 * Types for the Create Table feature.
 *
 * The wire types are generated from `crates/seaquel-types` (see `./generated`);
 * edit them there and run `npm run types:gen`.
 * @module types/create-table
 */

import type { CreateTableDefinition } from "./generated/CreateTableDefinition";

export type { ColumnCategory } from "./generated/ColumnCategory";
export type { ColumnTypeInfo } from "./generated/ColumnTypeInfo";
export type { CreateTableColumn } from "./generated/CreateTableColumn";
export type { CreateTableIndex } from "./generated/CreateTableIndex";
export type { CreateTableForeignKey } from "./generated/CreateTableForeignKey";
export type { CreateTableDefinition } from "./generated/CreateTableDefinition";

/**
 * Tab state for the Create Table editor.
 */
export interface CreateTableTab {
  id: string;
  connectionId: string;
  name: string;
  tableDefinition: CreateTableDefinition;
  generatedSql?: string;
  /** True when editing an existing table rather than creating a new one */
  isEditMode?: boolean;
  /** The original definition when editing, used to compute ALTER TABLE diffs */
  originalDefinition?: CreateTableDefinition;
}
