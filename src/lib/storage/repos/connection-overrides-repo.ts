import type { SqliteDatabase } from "../sqlite-types";
import type { PersistedConnectionOverride } from "../client";
import { createRepo, col, nullable, bool } from "../create-repo";

const _connectionOverridesRepo = createRepo<PersistedConnectionOverride>({
  table: "connection_overrides",
  id: "sharedConnectionId",
  columns: {
    sharedConnectionId: col("shared_connection_id"),
    username: nullable("username"),
    hostOverride: nullable("host_override"),
    portOverride: nullable("port_override"),
    savePassword: bool("save_password"),
    saveSshPassword: bool("save_ssh_password"),
    saveSshKeyPassphrase: bool("save_ssh_key_passphrase"),
  },
});

export const connectionOverridesRepo = {
  load(
    db: SqliteDatabase,
    sharedConnectionId: string,
  ): Promise<PersistedConnectionOverride | null> {
    return _connectionOverridesRepo.loadOneBy(db, "shared_connection_id = ?", [sharedConnectionId]);
  },
  loadAll(db: SqliteDatabase): Promise<PersistedConnectionOverride[]> {
    return _connectionOverridesRepo.loadAll(db);
  },
  save(db: SqliteDatabase, override: PersistedConnectionOverride): Promise<void> {
    return _connectionOverridesRepo.save(db, override);
  },
  remove(db: SqliteDatabase, sharedConnectionId: string): Promise<void> {
    return _connectionOverridesRepo.remove(db, sharedConnectionId);
  },
};
