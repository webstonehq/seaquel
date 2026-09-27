/**
 * The command lines and config snippets the MCP settings panel shows for
 * `seaquel-cli mcp`. Connections and projects are named by id: ids are stable
 * and plain, while names can hold anything and can change.
 */

export type ShellKind = "posix" | "powershell";

export interface McpSelection {
  /** Projects exposed whole (`--project <id>`). */
  projectIds: string[];
  /** Single connections (`--connection <id>`). */
  connectionIds: string[];
}

/** The server name the snippets register under. */
export const MCP_SERVER_NAME = "seaquel";

/** The arguments after the binary: `mcp`, then projects, then connections. */
export function mcpArgs(selection: McpSelection): string[] {
  const args = ["mcp"];
  for (const id of selection.projectIds) args.push("--project", id);
  for (const id of selection.connectionIds) args.push("--connection", id);
  return args;
}

// No `=` (zsh expands a leading one) and no `~`, `*`, `$` or spaces.
const POSIX_PLAIN = /^[A-Za-z0-9_@%+:,./-]+$/;

/** One word for a POSIX shell: left bare when plain, else single-quoted with `'` as `'\''`. */
export function posixQuote(word: string): string {
  if (POSIX_PLAIN.test(word)) return word;
  return `'${word.replaceAll("'", `'\\''`)}'`;
}

// Bare words: no `,` (it builds an array), `@`, `$`, `#`, quotes, parentheses
// or spaces, and no leading `-`. `--connection`-style flags stay bare, but
// `--` alone is quoted: PowerShell takes a bare `--` as the end of its own
// parameters and drops it when calling a script such as npm's `claude.ps1`.
const POWERSHELL_PLAIN = /^[A-Za-z0-9_:./\\][A-Za-z0-9_:./\\-]*$/;
const POWERSHELL_FLAG = /^--[A-Za-z][A-Za-z0-9-]*$/;

/**
 * One word for PowerShell: left bare when plain, else single-quoted. Inside
 * single quotes nothing expands; a quote is doubled, and PowerShell counts the
 * typographic single quotes (U+2018-U+201B) as quotes too.
 */
export function powershellQuote(word: string): string {
  if (POWERSHELL_PLAIN.test(word) || POWERSHELL_FLAG.test(word)) return word;
  return `'${word.replace(/['\u2018\u2019\u201A\u201B]/g, "$&$&")}'`;
}

export function quoteWord(word: string, shell: ShellKind): string {
  return shell === "posix" ? posixQuote(word) : powershellQuote(word);
}

/** The command to run the server by hand. PowerShell needs `&` to run a quoted path. */
export function commandLine(binary: string, selection: McpSelection, shell: ShellKind): string {
  const words = [binary, ...mcpArgs(selection)].map((w) => quoteWord(w, shell));
  if (shell === "powershell" && words[0].startsWith("'")) words[0] = `& ${words[0]}`;
  return words.join(" ");
}

/** `claude mcp add seaquel -- <binary> mcp …` for Claude Code. */
export function claudeCodeCommand(
  binary: string,
  selection: McpSelection,
  shell: ShellKind,
): string {
  const words = ["claude", "mcp", "add", MCP_SERVER_NAME, "--", binary, ...mcpArgs(selection)];
  return words.map((w) => quoteWord(w, shell)).join(" ");
}

/** Claude Desktop's `claude_desktop_config.json` entry, as pretty JSON. */
export function claudeDesktopConfig(binary: string, selection: McpSelection): string {
  const config = {
    mcpServers: {
      [MCP_SERVER_NAME]: {
        command: binary,
        args: mcpArgs(selection),
      },
    },
  };
  return JSON.stringify(config, null, 2);
}
