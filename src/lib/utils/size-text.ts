/** A size as the DuckDB install says it: `11.5 MB`, `641 KB`, `12 bytes` (decimal, as the TUI and release pages). */
export function sizeText(bytes: number): string {
  if (bytes >= 1_000_000) return `${(bytes / 1_000_000).toFixed(1)} MB`;
  if (bytes >= 1_000) return `${Math.ceil(bytes / 1_000)} KB`;
  return `${bytes} bytes`;
}
