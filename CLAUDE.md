# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Seaquel is a desktop database client built with Tauri 2 + SvelteKit 5 + TypeScript. It currently supports PostgreSQL connections via the `tauri-plugin-sql` plugin (with the `postgres` feature enabled).

## Development Commands

```bash
# Start development (frontend + Tauri)
npm run tauri dev

# Build production app
npm run tauri build

# Type checking
npm run check

# Type checking (watch mode)
npm run check:watch
```

## Architecture

### Frontend (src/)
- **SvelteKit 5** with static adapter (SSR disabled for Tauri)
- **Svelte 5 runes** (`$state`, `$derived`, `$props`) for reactivity
- **Tailwind CSS v4** for styling
- **bits-ui** for accessible UI components (shadcn-svelte pattern)

### Backend (src-tauri/)
- **Tauri 2** with Rust backend
- Plugins: `tauri-plugin-sql` (PostgreSQL), `tauri-plugin-store` (persistence), `tauri-plugin-updater`

### State Management
All app state is managed through a single reactive class in `src/lib/hooks/database.svelte.ts`:
- `UseDatabase` class uses Svelte 5 runes for reactivity
- Exposed via Svelte context (`setDatabase`/`useDatabase` pattern)
- Handles: connections, query tabs, schema tabs, query history, saved queries, AI messages

### Key Components (src/lib/components/)
- `query-editor.svelte` - SQL query editor with tab support
- `table-viewer.svelte` - Schema browser with table/column/index details
- `connection-dialog.svelte` - Database connection management
- `sidebar-left.svelte` / `sidebar-right.svelte` - Navigation sidebars
- UI components follow shadcn-svelte structure in `src/lib/components/ui/`

### Data Types (src/lib/types.ts)
Core interfaces: `DatabaseConnection`, `SchemaTable`, `QueryTab`, `QueryResult`, `SavedQuery`, `QueryHistoryItem`

## Configuration Files
- `src-tauri/tauri.conf.json` - Tauri app configuration
- `svelte.config.js` - SvelteKit config with static adapter
- `vite.config.js` - Vite bundler config

## Tools
Use 'bd' for task tracking

You are able to use the Svelte MCP server, where you have access to comprehensive Svelte 5 and SvelteKit documentation. Here's how to use the available tools effectively:

## Available MCP Tools:

### 1. list-sections

Use this FIRST to discover all available documentation sections. Returns a structured list with titles, use_cases, and paths.
When asked about Svelte or SvelteKit topics, ALWAYS use this tool at the start of the chat to find relevant sections.

### 2. get-documentation

Retrieves full documentation content for specific sections. Accepts single or multiple sections.
After calling the list-sections tool, you MUST analyze the returned documentation sections (especially the use_cases field) and then use the get-documentation tool to fetch ALL documentation sections that are relevant for the user's task.

### 3. svelte-autofixer

Analyzes Svelte code and returns issues and suggestions.
You MUST use this tool whenever writing Svelte code before sending it to the user. Keep calling it until no issues or suggestions are returned.

### 4. playground-link

Generates a Svelte Playground link with the provided code.
After completing the code, ask the user if they want a playground link. Only call this tool after user confirmation and NEVER if code was written to files in their project.