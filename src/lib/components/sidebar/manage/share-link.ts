/** What a share link is built from: Core's stored paths (phase 5e). */
export interface ShareResource {
  /** A shared query's or dashboard's file, relative to the repo. */
  sharedPath?: string;
  /** A linked connection's template: `<repoId>:<path>`. */
  sharedConnectionId?: string;
}

/** Whether a connection is shared with the repo: it has a template link. */
export function isSharedConnection(c: { sharedConnectionId?: string }): boolean {
  return !!c.sharedConnectionId;
}

/** The template path inside a `sharedConnectionId` (`<repoId>:.seaquel/…`). */
export function templatePath(sharedConnectionId: string | undefined): string | undefined {
  if (!sharedConnectionId) return undefined;
  const at = sharedConnectionId.indexOf(":.seaquel/");
  return at === -1 ? undefined : sharedConnectionId.slice(at + 1);
}
