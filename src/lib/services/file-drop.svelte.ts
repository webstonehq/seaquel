import { toast } from "svelte-sonner";
import { showErrorUnlessShown } from "$lib/errors";
import type { useDatabase } from "$lib/hooks/database.svelte.js";
import { DEFAULT_PROJECT_ID } from "$lib/types";
import { DuckdbHelperDeclined } from "$lib/core/duckdb-helper";

type Database = ReturnType<typeof useDatabase>;

const QUICK_QUERY_CONNECTION_NAME = "Quick Query (DuckDB)";

const SUPPORTED_DATA_EXTENSIONS: Record<string, string> = {
  parquet: "read_parquet",
  csv: "read_csv",
  json: "read_json_auto",
  xlsx: "read_xlsx",
  xls: "read_xlsx",
};

const DATABASE_EXTENSIONS = ["duckdb", "db"];

function getFileExtension(path: string): string {
  return path.split(".").pop()?.toLowerCase() ?? "";
}

function escapeSqlString(value: string): string {
  return value.replace(/'/g, "''");
}

function generateReadQuery(path: string, readFn: string): string {
  return `SELECT * FROM ${readFn}('${escapeSqlString(path)}')`;
}

function getFileName(path: string): string {
  return path.split("/").pop() ?? path;
}

/** Reactive state shared with the overlay component */
export const fileDropState = $state({
  visible: false,
  fileNames: [] as string[],
});

async function getOrCreateQuickDuckDBConnection(db: Database): Promise<string> {
  const projectId = db.state.activeProjectId || DEFAULT_PROJECT_ID;

  const existing = db.state.connections.find(
    (c: { name: string; projectId: string }) =>
      c.name === QUICK_QUERY_CONNECTION_NAME && c.projectId === projectId,
  );

  if (existing) {
    if (existing.providerConnectionId) {
      return existing.id;
    }
    await db.connections.reconnect(existing.id, {
      name: existing.name,
      type: "duckdb",
      host: "",
      port: 0,
      databaseName: ":memory:",
      username: "",
      password: "",
    });
    return existing.id;
  }

  const connectionId = await db.connections.add({
    name: QUICK_QUERY_CONNECTION_NAME,
    type: "duckdb",
    host: "",
    port: 0,
    databaseName: ":memory:",
    username: "",
    password: "",
    projectId,
  });

  return connectionId;
}

async function handleDataFile(
  path: string,
  readFn: string,
  db: Database,
  connectionId: string,
): Promise<void> {
  const fileName = getFileName(path);
  const query = generateReadQuery(path, readFn);

  if (db.state.activeConnectionId !== connectionId) {
    db.connections.setActive(connectionId);
  }

  const tabId = db.queryTabs.add(fileName, query);
  if (!tabId) return;

  try {
    await db.queries.execute(tabId);
  } catch {
    // Query errors are displayed in the results pane
  }
}

/**
 * Connect a dropped DuckDB file. Answers `false` when the user declined to
 * install DuckDB support, so the rest of the drop doesn't ask again.
 */
async function handleDatabaseFile(path: string, db: Database): Promise<boolean> {
  const fileName = getFileName(path);
  try {
    await db.connections.add({
      name: fileName,
      type: "duckdb",
      host: "",
      port: 0,
      databaseName: path,
      username: "",
      password: "",
    });
    toast.success(`Connected to ${fileName}`);
  } catch (error) {
    if (error instanceof DuckdbHelperDeclined) return false;
    showErrorUnlessShown(error, (message) => `Failed to connect to ${fileName}: ${message}`);
  }
  return true;
}

export async function handleFileDrop(paths: string[], db: Database): Promise<void> {
  const dataFiles: { path: string; readFn: string }[] = [];
  const dbFiles: string[] = [];
  const unsupported: string[] = [];

  for (const path of paths) {
    const ext = getFileExtension(path);
    const readFn = SUPPORTED_DATA_EXTENSIONS[ext];

    if (readFn) {
      dataFiles.push({ path, readFn });
    } else if (DATABASE_EXTENSIONS.includes(ext)) {
      dbFiles.push(path);
    } else {
      unsupported.push(getFileName(path));
    }
  }

  if (unsupported.length > 0) {
    toast.info(`Unsupported file type: ${unsupported.join(", ")}`);
  }

  // Everything in a drop is DuckDB work: one decline of DuckDB support's
  // install ends it, instead of asking once per file.
  let declined = false;
  for (const path of dbFiles) {
    if (!(await handleDatabaseFile(path, db))) {
      declined = true;
      break;
    }
  }

  if (dataFiles.length > 0 && !declined) {
    try {
      const connectionId = await getOrCreateQuickDuckDBConnection(db);
      for (const { path, readFn } of dataFiles) {
        await handleDataFile(path, readFn, db, connectionId);
      }
    } catch (error) {
      showErrorUnlessShown(error, (message) => `Failed to create DuckDB connection: ${message}`);
    }
  }
}

export async function setupFileDropListener(db: Database): Promise<() => void> {
  const { listen } = await import("@tauri-apps/api/event");

  const unlistenDrop = await listen<string[]>("file-drop", async (event) => {
    fileDropState.visible = false;
    fileDropState.fileNames = [];
    await handleFileDrop(event.payload, db);
  });

  const unlistenHover = await listen<string[]>("file-drop-hover", (event) => {
    fileDropState.fileNames = event.payload.map(getFileName);
    fileDropState.visible = true;
  });

  const unlistenLeave = await listen("file-drop-leave", () => {
    fileDropState.visible = false;
    fileDropState.fileNames = [];
  });

  return () => {
    unlistenDrop();
    unlistenHover();
    unlistenLeave();
  };
}
