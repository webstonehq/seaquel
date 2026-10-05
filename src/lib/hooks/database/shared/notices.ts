/**
 * A sync's notices, worded for the user. Core names rows by
 * id and files by their path relative to `.seaquel/`; the page names the
 * rows it holds. Core already applies the once-per-session rule, so each
 * notice that arrives is said.
 */
import { toast } from "svelte-sonner";
import { m } from "$lib/paraglide/messages.js";
import { errorToast } from "$lib/utils/toast";
import { errorCode } from "$lib/core/client";
import type { DatabaseState } from "../state.svelte.js";
import { PROJECT_ALREADY_LINKED, REPO_CONFLICTED, REPO_IN_USE } from "./types";
import type {
  ProjectFailure,
  ReplacedValues,
  SharedKind,
  SkipReason,
  SyncNotice,
  SyncReport,
} from "./types";

/** At most this many notices are toasted one by one; the rest are counted. */
const MAX_NOTICE_TOASTS = 5;

type NameLookup = Pick<DatabaseState, "queriesByProject" | "dashboardsByProject" | "connections">;

/** The name of row `id` of `kind`, as the page holds it, else the id. */
function nameOf(state: NameLookup, kind: SharedKind | null, id: string): string {
  const query = () =>
    Object.values(state.queriesByProject)
      .flat()
      .find((q) => q.id === id)?.name;
  const dashboard = () =>
    Object.values(state.dashboardsByProject)
      .flat()
      .find((d) => d.id === id)?.name;
  const connection = () => state.connections.find((c) => c.id === id)?.name;
  switch (kind) {
    case "savedQuery":
      return query() ?? id;
    case "dashboard":
      return dashboard() ?? id;
    case "connection":
      return connection() ?? id;
    default:
      return query() ?? dashboard() ?? connection() ?? id;
  }
}

/** The kind of row an id names, by Core's id prefixes. */
function kindOfId(id: string): SharedKind | null {
  if (id.startsWith("saved-")) return "savedQuery";
  if (id.startsWith("dashboard-")) return "dashboard";
  if (id.startsWith("conn-")) return "connection";
  return null;
}

export function skipReason(why: SkipReason): string {
  switch (why) {
    case "symlink":
      return m.shared_skip_symlink();
    case "unreadable":
      return m.shared_skip_unreadable();
    case "notUtf8":
      return m.shared_skip_not_utf8();
    case "tooLarge":
      return m.shared_skip_too_large();
    case "tooMany":
      return m.shared_skip_too_many();
    case "doesNotParse":
      return m.shared_skip_does_not_parse();
    case "invalid":
      return m.shared_skip_invalid();
  }
}

/** The values a template replaced, as `field value` pairs, the fields worded. */
function replacedText(replaced: ReplacedValues): string {
  const labels: Record<keyof ReplacedValues, () => string> = {
    name: m.shared_field_name,
    host: m.shared_field_host,
    port: m.shared_field_port,
    databaseName: m.shared_field_database,
    sslMode: m.shared_field_ssl_mode,
    sshHost: m.shared_field_ssh_host,
    sshPort: m.shared_field_ssh_port,
  };
  return (Object.keys(labels) as (keyof ReplacedValues)[])
    .filter((key) => key in replaced)
    .map((key) => `${labels[key]()} ${replaced[key] ?? "—"}`)
    .join(", ");
}

/** One notice as the user reads it. */
export function noticeText(state: NameLookup, notice: SyncNotice): string {
  switch (notice.type) {
    case "conflict": {
      const name = nameOf(state, notice.kind, notice.id);
      if (notice.kind === "connection") {
        const values = notice.replaced ? replacedText(notice.replaced) : "";
        return values
          ? m.shared_notice_conflict_connection({ name, values })
          : m.shared_notice_conflict({ name });
      }
      return m.shared_notice_conflict({ name });
    }
    case "removedInRepo":
      return m.shared_notice_removed_in_repo({ name: nameOf(state, notice.kind, notice.id) });
    case "nameTaken": {
      const kind = kindOfId(notice.takenBy);
      const name = nameOf(state, kind, notice.takenBy);
      switch (kind) {
        case "savedQuery":
          return m.shared_notice_name_taken_saved_query({ path: notice.path, name });
        case "dashboard":
          return m.shared_notice_name_taken_dashboard({ path: notice.path, name });
        case "connection":
          return m.shared_notice_name_taken_connection({ path: notice.path, name });
        default:
          return m.shared_notice_name_taken({ path: notice.path });
      }
    }
    case "unpaired":
      return m.shared_notice_unpaired({
        path: notice.path,
        name: nameOf(state, kindOfId(notice.claims), notice.claims),
      });
    case "skipped":
      return m.shared_notice_skipped({ path: notice.path, reason: skipReason(notice.why) });
    case "templateTypeChanged":
      return m.shared_notice_template_type_changed({
        name: nameOf(state, "connection", notice.id),
        type: notice.templateType,
        imported: nameOf(state, "connection", notice.imported),
      });
  }
}

/** A project a repo's sync or an import couldn't finish, as the user reads it. */
export function failureText(
  state: Pick<DatabaseState, "projects">,
  failure: ProjectFailure,
): string {
  const name =
    (failure.projectId && state.projects.find((p) => p.id === failure.projectId)?.name) ||
    failure.dir ||
    "";
  // A folder a project here already links isn't imported again.
  const message =
    failure.code === PROJECT_ALREADY_LINKED
      ? m.shared_import_dir_already_linked()
      : failure.message;
  return m.shared_project_sync_failed({ name, message });
}

/**
 * Say a sync's notices and failures: each notice as a toast (a conflict or
 * a removal as a warning, the rest as information), at most
 * `MAX_NOTICE_TOASTS` of them, then how many more there were.
 */
export function sayReport(state: NameLookup & Pick<DatabaseState, "projects">, report: SyncReport) {
  for (const failure of report.failures ?? []) errorToast(failureText(state, failure));
  const notices = report.notices;
  for (const notice of notices.slice(0, MAX_NOTICE_TOASTS)) {
    const text = noticeText(state, notice);
    if (notice.type === "conflict" || notice.type === "removedInRepo") toast.warning(text);
    else toast.info(text);
  }
  if (notices.length > MAX_NOTICE_TOASTS) {
    toast.info(m.shared_notice_more({ count: notices.length - MAX_NOTICE_TOASTS }));
  }
}

/** A refused `shared` call, worded for the user (Core's message otherwise). */
export function sharedErrorMessage(error: unknown): string {
  switch (errorCode(error)) {
    case PROJECT_ALREADY_LINKED:
      return m.shared_error_project_already_linked();
    case REPO_CONFLICTED:
      return m.shared_error_repo_conflicted();
    case REPO_IN_USE:
      return m.shared_error_repo_in_use();
    default: {
      const code = errorCode(error);
      const message = error instanceof Error ? error.message : String(error);
      return code && message.startsWith(`${code}: `) ? message.slice(code.length + 2) : message;
    }
  }
}

/** A refused `shared` call as an `Error` with the user's wording and Core's code. */
export function sharedError(error: unknown): Error {
  return Object.assign(new Error(sharedErrorMessage(error), { cause: error }), {
    code: errorCode(error),
  });
}
