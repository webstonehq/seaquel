import { toast } from "svelte-sonner";
import { errorToast } from "$lib/utils/toast";
import { save } from "@tauri-apps/plugin-dialog";
import { writeTextFile } from "@tauri-apps/plugin-fs";
import { format as formatSQL } from "sql-formatter";
import {
  formatConfig,
  getExportContent,
  insertTarget,
  type ExportFormat,
} from "$lib/utils/export-formats.js";
import { m } from "$lib/paraglide/messages.js";
import { getEngineClient } from "$lib/engine";
import type { QueryEditorContext } from "./types.js";

export function createSaveFormatExport(ctx: QueryEditorContext) {
  const { db } = ctx;

  let showSaveDialog = $state(false);
  let showSaveAsDialog = $state(false);

  function handleSave() {
    const activeTab = ctx.getActiveTab();
    if (!activeTab?.query.trim()) return;
    if (activeTab.queryId) {
      const savedQuery = db.state.projectQueries.find((q) => q.id === activeTab.queryId);
      if (savedQuery) {
        db.savedQueries.saveQuery(
          savedQuery.name,
          activeTab.query,
          activeTab.id,
          savedQuery.parameters,
        );
        toast.success(m.save_query_success());
        return;
      }
    }
    showSaveDialog = true;
  }

  function handleSaveAs() {
    const activeTab = ctx.getActiveTab();
    if (!activeTab?.query.trim()) return;
    showSaveAsDialog = true;
  }

  function handleFormat() {
    const activeTab = ctx.getActiveTab();
    const activeTabId = ctx.getActiveTabId();
    if (!activeTab?.query.trim() || !activeTabId) return;
    try {
      const formatted = formatSQL(activeTab.query, {
        language: "postgresql",
        tabWidth: 2,
        keywordCase: "upper",
      });
      db.queryTabs.updateContent(activeTabId, formatted);
    } catch {
      errorToast(m.query_format_failed());
    }
  }

  function getContent(format: ExportFormat): string {
    const activeResult = ctx.getActiveResult();
    if (!activeResult) return format === "json" ? "[]" : "";
    const connection = db.state.activeConnection;
    // The INSERT target: the result's source table when every column is its
    // own, else a placeholder.
    const source = insertTarget(activeResult, db.state.activeSchema);
    const tableName =
      connection && source
        ? getEngineClient(connection, db.state).qualifiedTable(source.schema, source.name)
        : undefined;
    return getExportContent(
      format,
      activeResult.columns,
      activeResult.rows,
      tableName,
      connection?.type,
    );
  }

  async function handleExport(format: ExportFormat) {
    if (!ctx.getActiveResult()) return;

    const config = formatConfig[format];
    const timestamp = new Date().toISOString().slice(0, 19).replace(/[:-]/g, "");
    const defaultName = `query_results_${timestamp}.${config.extension}`;
    const filters = [{ name: config.name, extensions: [config.extension] }];

    const filePath = await save({ defaultPath: defaultName, filters });
    if (!filePath) return;

    const content = getContent(format);
    await writeTextFile(filePath, content);
  }

  async function handleCopy(format: ExportFormat) {
    if (!ctx.getActiveResult()) return;

    const content = getContent(format);
    const formatNames: Record<ExportFormat, string> = {
      csv: "CSV",
      json: "JSON",
      sql: "SQL INSERT",
      markdown: "Markdown",
    };

    try {
      await navigator.clipboard.writeText(content);
      toast.success(m.query_copied_to_clipboard({ format: formatNames[format] }));
    } catch {
      errorToast(m.query_copy_failed());
    }
  }

  return {
    get showSaveDialog() {
      return showSaveDialog;
    },
    set showSaveDialog(v: boolean) {
      showSaveDialog = v;
    },
    get showSaveAsDialog() {
      return showSaveAsDialog;
    },
    set showSaveAsDialog(v: boolean) {
      showSaveAsDialog = v;
    },

    handleSave,
    handleSaveAs,
    handleFormat,
    handleExport,
    handleCopy,
  };
}
