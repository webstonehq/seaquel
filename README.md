<div align="center">

# Seaquel

**Explore, query, and visualize your databases — all in one app.**

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Free for personal use](https://img.shields.io/badge/Free-for%20personal%20use-blue.svg)](https://seaquel.app/pricing)
[![Version](https://img.shields.io/github/v/release/webstonehq/seaquel)](https://github.com/webstonehq/seaquel/releases)
[![GitHub Stars](https://img.shields.io/github/stars/webstonehq/seaquel)](https://github.com/webstonehq/seaquel/stargazers)
[![Discord](https://img.shields.io/discord/1452421515164385436?label=Discord&logo=discord&logoColor=white)](https://seaquel.app/discord)
[![Try Demo](https://img.shields.io/badge/Try-Live%20Demo-brightgreen)](https://seaquel.app/demo)

![Seaquel Screenshot](https://seaquel.app/product-screenshot.png)

Works with 6 database engines. No account required. Open source, free for personal use.<br>
[Try it in your browser](https://seaquel.app/demo) in seconds.

</div>

## Features

### Query & Edit

- **SQL editor** — Syntax highlighting, formatting, parameter support, and Monaco-based editing
- **Inline result editing** — INSERT, UPDATE, and DELETE rows directly from the results table
- **Visual query builder** — Drag-and-drop canvas for building queries without SQL
- **AI assistant** — Get help writing and understanding SQL queries
- **SQL learning sandbox** — Interactive challenges to practice SQL

### Explore & Visualize

- **Schema browser** — Explore tables, columns, indexes, constraints, and more
- **Entity Relationship Diagrams** — Auto-generated ERDs with PNG/SVG export
- **EXPLAIN/ANALYZE visualizer** — Understand query plans with hot path detection
- **Database statistics** — Dashboard with table sizes, row counts, and index usage
- **Data visualization** — Bar, line, pie, and scatter charts from query results

### Collaborate & Share

- **CSV/JSON export** — Export query results to CSV or JSON
- **Query sharing** — Share queries via Git repositories
- **Connection import** — Import connections from DBeaver and TablePlus
- **Multi-project** — Organize connections and queries across projects

### Customize

- **Themes** — Light and dark modes with a built-in theme editor
- **Internationalization** — Available in English, Spanish, German, French, Arabic, and Korean
- **Command palette** — Quick access to all actions via keyboard
- **SSH tunneling** — Connect securely through SSH tunnels
- **Auto-updates** — Stay current with automatic update notifications

## Comparison with Alternatives

|                       |      Seaquel       |      DBeaver       |     TablePlus      |      DataGrip      |      pgAdmin       |
| --------------------- | :----------------: | :----------------: | :----------------: | :----------------: | :----------------: |
| Open source           | :white_check_mark: | :white_check_mark: |        :x:         |        :x:         | :white_check_mark: |
| Multi-database        | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: |        :x:         |
| Browser demo          | :white_check_mark: |        :x:         |        :x:         |        :x:         |        :x:         |
| ERD generation        | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: |        :x:         |
| Visual query builder  | :white_check_mark: | :white_check_mark: |        :x:         |        :x:         | :white_check_mark: |
| EXPLAIN visualizer    | :white_check_mark: | :white_check_mark: |        :x:         | :white_check_mark: | :white_check_mark: |
| Free for personal use | :white_check_mark: | :white_check_mark: |        :x:         |        :x:         | :white_check_mark: |
| Lightweight           | :white_check_mark: |        :x:         | :white_check_mark: |        :x:         | :white_check_mark: |

## Installation

Download Seaquel for your platform from [seaquel.app/download](https://seaquel.app/download).

| Platform | Architectures                         |
| -------- | ------------------------------------- |
| macOS    | Intel (x86_64), Apple Silicon (ARM64) |
| Linux    | x86_64, ARM64                         |
| Windows  | x86_64, ARM64                         |

Want to try it first? Check out the [browser demo](https://seaquel.app/demo) (powered by DuckDB WASM).

## Database Support

| Feature              |     PostgreSQL     |       MySQL        |      MariaDB       |       SQLite       |       MSSQL        |       DuckDB       |
| -------------------- | :----------------: | :----------------: | :----------------: | :----------------: | :----------------: | :----------------: |
| Connect & query      | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: |
| Schema browser       | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: |
| EXPLAIN visualizer   | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: |
| ERD generation       | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: |
| Inline editing       | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: |
| Statistics dashboard | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: |

## Community

- [Discord](https://seaquel.app/discord) — Chat, ask questions, share feedback
- [GitHub Issues](https://github.com/webstonehq/seaquel/issues) — Bug reports and feature requests

## Contributing

Contributions are welcome! Here's how to get started:

1. Browse the [open issues](https://github.com/webstonehq/seaquel/issues) to find something to work on
2. For larger changes, open an issue first to discuss your approach
3. Fork the repo, create a branch, and submit a pull request

## Development

### Prerequisites

Use [mise](https://mise.jdx.dev/) to install the required toolchain:

```bash
mise install
```

This installs Node.js and Rust as defined in `mise.toml`.

### Setup

```bash
git clone https://github.com/webstonehq/seaquel.git
cd seaquel
npm install
```

### Commands

```bash
npm run tauri:dev       # Start development, note "tauri:dev" vs the default "tauri dev"
npm run tauri build     # Build production app
npm run check           # Type checking
npm run check:watch     # Type checking (watch mode)
```

## Tech Stack

- [Tauri](https://tauri.app/) — Native app shell with Rust backend
- [SvelteKit](https://svelte.dev/docs/kit) — App framework
- [Svelte](https://svelte.dev/) — UI with runes-based reactivity
- [TypeScript](https://www.typescriptlang.org/) — Type safety
- [Tailwind CSS](https://tailwindcss.com/) — Styling
- [Rust](https://www.rust-lang.org/) — Backend plugins and system integration

## License

Seaquel splits its source from the binaries we distribute.

**Source code** — [MIT](LICENSE). View it, modify it, redistribute it, and
build and run your own binaries, including at work, with no license from us.
The MIT grant does not extend to the Seaquel name, logo, or branding, so please
don't ship your builds under them.

**Official desktop binaries** — the builds on
[seaquel.app/download](https://seaquel.app/download), GitHub Releases, and the
in-app updater are free for personal, non-commercial use. Using them for work
requires a commercial license: see [pricing](https://seaquel.app/pricing).

**Seaquel Cloud and the self-host image** — a subscription is required for all
use, personal included.

Full terms: [seaquel.app/terms](https://seaquel.app/terms).
