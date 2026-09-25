/**
 * Splits a script from `EngineClient.createTable`/`alterTable` the way the
 * table editor runs it: pieces separated by `;\n`, without empty ones.
 *
 * A piece starting with `--` is a note, not a statement: the Rust dialects
 * turn edits the engine can't make (SQLite's type or default changes, added
 * foreign keys, dropping a constraint's index) into `-- …` lines after every
 * statement. `notes` holds their text, one per line, without the `--`.
 */
export function splitDdlScript(sql: string): { statements: string[]; notes: string[] } {
  const statements: string[] = [];
  const notes: string[] = [];
  for (const piece of sql.split(";\n")) {
    const trimmed = piece.trim();
    if (!trimmed) continue;
    if (trimmed.startsWith("--")) {
      for (const line of trimmed.split("\n")) {
        const text = line.trim().replace(/^--\s*/, "");
        if (text) notes.push(text);
      }
    } else {
      statements.push(trimmed.endsWith(";") ? trimmed : `${trimmed};`);
    }
  }
  return { statements, notes };
}
