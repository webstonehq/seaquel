import type { SqliteDatabase } from "../sqlite-types";
import type { PersistedDashboard } from "../client";
import { createRepo, col, nullable, bool } from "../create-repo";
import type { ColumnDef } from "../create-repo";

/** Passes through on read; coerces `undefined` to `null` on write. */
function nullablePassthrough(dbColumn: string): ColumnDef {
  return {
    dbColumn,
    toDb: (v) => v ?? null,
    fromDb: (v) => v,
  };
}

const _dashboardsRepo = createRepo<PersistedDashboard>({
  table: "dashboards",
  id: "id",
  columns: {
    id: col("id"),
    projectId: col("project_id"),
    name: col("name"),
    viewport: col("viewport"),
    widgets: col("widgets"),
    dateFilter: nullablePassthrough("date_filter"),
    starred: bool("starred"),
    shared: bool("shared"),
    description: nullable("description"),
    createdAt: col("created_at"),
    updatedAt: col("updated_at"),
  },
});

export const dashboardsRepo = {
  loadByProject(db: SqliteDatabase, projectId: string): Promise<PersistedDashboard[]> {
    return _dashboardsRepo.loadBy(db, "project_id = ?", [projectId]);
  },
  save(db: SqliteDatabase, dashboard: PersistedDashboard): Promise<void> {
    return _dashboardsRepo.save(db, dashboard);
  },
  remove(db: SqliteDatabase, id: string): Promise<void> {
    return _dashboardsRepo.remove(db, id);
  },
  removeByProject(db: SqliteDatabase, projectId: string): Promise<void> {
    return _dashboardsRepo.removeBy(db, "project_id = ?", [projectId]);
  },
};
