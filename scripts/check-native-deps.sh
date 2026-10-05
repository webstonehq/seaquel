#!/usr/bin/env bash
# CI's "Native binaries' dependencies" step (ci.yml, `rust` job), here so it
# can be run locally as written: `bash scripts/check-native-deps.sh`.
#
# Phase 7a, Decisions 2 and 22: each terminal binary links only its own
# extras. The TUI has no MCP server (rmcp), git, imports (plist) or license
# client. The CLI's reqwest ban went with `seaquel-cli duckdb install` (the
# DuckDB helper plan, Q4 A, Task 8): it downloads the helper through
# seaquel-http.
#
# No native binary links DuckDB (the DuckDB helper plan, Decision 11; the
# desktop DuckDB helper plan, Decision 20): the TUI, the CLI and the desktop
# app (`seaquel`, src-tauri) all run DuckDB in the `seaquel-duckdb` helper,
# so no crate named like `duckdb` but the engine crate (whose `remote`
# driver is plain Rust) may be in their trees; that catches `duckdb` and
# `libduckdb-sys`. Normal dependencies, as each binary builds.
set -euo pipefail

# One list per binary, built the same way; a failing `cargo tree` fails the
# step (pipefail, and `set -e` on the assignment). `--target all`: every
# platform's dependencies, not only this runner's (a Windows- or macOS-only
# crate counts too).
deps() { cargo tree -p "$1" -e normal --target all --prefix none "${@:2}" | sed 's/ .*//' | sort -u; }

found=0
tui=$(deps seaquel-tui)
test -n "$tui"
for name in rmcp git2 libgit2-sys plist seaquel-mcp seaquel-rpc seaquel-license; do
  if grep -qx -- "$name" <<<"$tui"; then
    echo "::error::seaquel-tui depends on $name"
    found=1
  fi
done

for bin in seaquel-tui seaquel-cli seaquel; do
  list=$(deps "$bin")
  test -n "$list"
  duck=$(grep -- duckdb <<<"$list" | grep -vx -- seaquel-engine-duckdb || true)
  if [ -n "$duck" ]; then
    echo "::error::$bin links DuckDB: ${duck//$'\n'/ }"
    found=1
  fi
done

# The remote driver on its own feature links no DuckDB either.
remote=$(deps seaquel-engine-duckdb --no-default-features --features remote)
test -n "$remote"
if grep -qx -- libduckdb-sys <<<"$remote"; then
  echo "::error::seaquel-engine-duckdb's remote feature links libduckdb-sys"
  found=1
fi
exit "$found"
