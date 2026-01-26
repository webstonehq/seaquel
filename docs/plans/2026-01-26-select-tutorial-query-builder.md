# Interactive SELECT Statement Tutorial - Design Document

## Overview

An interactive SQL query builder for teaching SELECT statements. Users drag tables onto a canvas, define columns, JOINs, filters, ordering, and limits visually while seeing the generated SQL update in real-time. Two-way sync allows editing SQL directly.

## Key Decisions

| Decision | Choice |
|----------|--------|
| Data source | Bundled SQLite sample database |
| Domain | E-commerce (customers, orders, products) |
| Column selection | Checkboxes on table nodes |
| JOIN creation | Drag between columns, smart defaults + edge label |
| Filters/Sort/Limit | Stacked panel below canvas |
| SQL editor | Two-way sync with Monaco |
| Lesson structure | Hybrid: guided challenges + sandbox mode |
| Challenge validation | Criteria checklist |

## Architecture

### Route Structure

```
/learn                    → Lesson list / dashboard
/learn/sandbox            → Free-form query builder playground
/learn/[lessonId]         → Guided lesson with challenges
```

### New Components

```
src/lib/components/query-builder/
├── canvas.svelte              # Main @xyflow/svelte canvas
├── table-node.svelte          # Custom node for tables
├── join-edge.svelte           # Custom edge for joins
├── table-palette.svelte       # Draggable table list
├── filter-panel.svelte        # WHERE/ORDER BY/LIMIT
├── sql-editor.svelte          # Monaco with two-way sync
├── challenge-card.svelte      # Challenge display + criteria
└── index.ts                   # Exports
```

### State Management

New `QueryBuilderState` class in `src/lib/hooks/query-builder.svelte.ts`:

- `tables` - Tables on canvas with positions and selected columns
- `joins` - Connections between tables with join type
- `filters` - WHERE conditions (column, operator, value, AND/OR)
- `orderBy` - Sort columns and direction
- `limit` - Row limit value
- `generatedSql` - Derived SQL string from visual state
- `parsedFromSql` - Reverse: parse SQL back to visual state

### Supporting Files

```
src/lib/tutorial/
├── schema.ts                  # Sample DB table definitions
├── lessons.ts                 # Lesson content & challenges
├── criteria.ts                # Validation functions
└── sql-parser.ts              # SQL ↔ visual state conversion

src/lib/types/
└── query-builder.ts           # TypeScript interfaces

src-tauri/resources/
└── tutorial.sqlite            # Bundled sample database
```

## Canvas & Table Nodes

### Table Palette

Left side of canvas shows available tables. Users drag from palette to canvas.

### Table Node Structure

```
┌─────────────────────────┐
│ 📦 products             │  ← Table name with icon
├─────────────────────────┤
│ ☑ id           INTEGER  │  ← Checkbox + column + type
│ ☑ name         TEXT     │
│ ☐ description  TEXT     │
│ ☑ price        DECIMAL  │
│ ☐ category_id  INTEGER  │  ← FK columns show link icon
│ ☐ created_at   DATETIME │
└─────────────────────────┘
```

- Header shows table name (draggable to move node)
- Each row: checkbox, column name, data type
- Checked columns included in SELECT
- Column rows have connection handles for creating joins

### Node Interactions

- Click checkbox → toggle column in SELECT
- Drag from column handle → start creating join edge
- Right-click node → context menu (Remove, Select all)

## Joins & Edges

### Creating Joins

Drag from column handle on Table A to column on Table B:

1. Auto-detect join type → defaults to INNER JOIN
2. Auto-detect ON condition → `orders.customer_id` → `customers.id`
3. Smart naming detection → `*_id` → `id` patterns

### Edge Appearance

- Solid line for INNER JOIN
- Dashed line for LEFT/RIGHT/FULL OUTER
- Clickable label shows join type
- Click label → dropdown to change type

### Join Type Options

- INNER JOIN (default)
- LEFT JOIN
- RIGHT JOIN
- FULL OUTER JOIN

## Filter Panel

Positioned below canvas, three stacked sections:

### WHERE Section

```
┌─────────────────────────────────────────────────────────────┐
│ ▼ WHERE                                              [+ Add] │
├─────────────────────────────────────────────────────────────┤
│  products.price    ▼  is greater than  ▼   100      [×]     │
│  AND ▼                                                       │
│  customers.country ▼  equals           ▼   "USA"    [×]     │
└─────────────────────────────────────────────────────────────┘
```

Operators adapt to column type:
- Text: equals, not equals, contains, starts with, ends with, is null
- Number: =, ≠, >, <, ≥, ≤, between, is null
- Date: =, before, after, between, is null

### ORDER BY Section

- Column dropdown
- Direction toggle: ASC / DESC
- Multiple columns, drag to reorder

### LIMIT Section

- Number input
- "No limit" checkbox

## Two-Way SQL Editor

### Layout

SQL editor sits right of canvas. Split view with canvas left, editor right, filter panel below both.

### Visual → SQL

When visual state changes, regenerate SQL and format with `sql-formatter`.

### SQL → Visual

When user edits SQL, debounce 500ms then:

1. Parse with `node-sql-parser`
2. Extract tables, columns, joins, where, order by, limit
3. Update visual state
4. Reposition nodes if needed

### Unsupported SQL

If SQL contains features not representable visually (subqueries, UNION, CTEs, functions):

```
⚠️ This query contains advanced features not supported by the visual builder.
   Visual editing disabled. [Reset to visual mode]
```

Canvas becomes read-only until user simplifies or resets.

## Guided Challenges

### Lesson Structure

Each lesson contains:
- Brief intro (2-3 paragraphs)
- 3-5 challenges of increasing difficulty
- Sandbox button for free practice

### Challenge Display

```
┌─────────────────────────────────────────────────────────────┐
│ Challenge 2 of 5                                            │
│                                                             │
│ "Find all products that cost more than $50, sorted by       │
│  price from highest to lowest."                             │
│                                                             │
│ ┌─────────────────────────────────────────────────────────┐ │
│ │ ✓ Selected columns from products table                  │ │
│ │ ✓ Includes 'name' column                                │ │
│ │ ✗ Has WHERE clause filtering by price                   │ │
│ │ ✗ Uses ORDER BY on price descending                     │ │
│ └─────────────────────────────────────────────────────────┘ │
│                                                             │
│ [Show Hint]                      [Skip] [Check Solution ▶]  │
└─────────────────────────────────────────────────────────────┘
```

### Criteria Validation

Criteria defined as checks:
- `hasTable("products")`
- `hasColumn("products", "name")`
- `hasWhere("products.price", ">", 50)`
- `hasOrderBy("products.price", "DESC")`
- `hasJoin("orders", "customers")`
- `resultCountAtLeast(5)`

Real-time updates as users build. All green = complete.

### Progression

- Complete challenge → unlock next
- Complete all in lesson → unlock next lesson
- Progress saved to local storage

## Sample Database Schema

### Tables

```
categories          products            reviews
├── id              ├── id              ├── id
├── name            ├── category_id ──► │ product_id
└── description     ├── name            ├── customer_id ──┐
                    ├── description     ├── rating        │
                    ├── price           ├── comment       │
                    ├── stock           └── created_at    │
                    └── created_at                        │
                                                          │
customers           orders              order_items       │
├── id ◄────────────┤ customer_id       ├── id            │
├── name            ├── id ◄────────────┤ order_id        │
├── email           ├── status          ├── product_id    │
├── country         ├── total           ├── quantity      │
└── created_at      └── created_at      └── unit_price    │
      ▲                                                   │
      └───────────────────────────────────────────────────┘
```

### Data Volume

- 5 categories
- 50 products
- 100 customers
- 500 orders
- 1500 order_items
- 200 reviews

~500KB SQLite file, bundled in `src-tauri/resources/`.

### Data Characteristics

- Dates spanning 2 years
- Prices $5-$500
- Mix of countries
- Some products with no reviews (LEFT JOIN examples)
- Some customers with no orders (OUTER JOIN examples)

## Dependencies

- `@xyflow/svelte` - Already installed
- `node-sql-parser` - Already installed
- `sql-formatter` - Already installed
- Monaco editor - Already installed

### Tauri Changes

Enable SQLite feature in `tauri-plugin-sql` alongside existing postgres feature.
