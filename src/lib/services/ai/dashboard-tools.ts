import type { DashboardWidget, KpiConfig, TextConfig } from "$lib/types/dashboard";
import type { ChartConfig } from "$lib/types/chart";

export interface DashboardGetResult {
  id: string;
  name: string;
  widgets: Array<{
    id: string;
    title: string;
    x: number;
    y: number;
    width: number;
    height: number;
    widgetType: string;
    query: string;
    chartConfig?: ChartConfig;
    kpiConfig?: KpiConfig;
    textConfig?: TextConfig;
  }>;
}

export interface DashboardCallbacks {
  onCreateDashboard: (name: string) => Promise<{ dashboardId: string } | null>;
  onAddWidget: (
    dashboardId: string,
    widget: Omit<DashboardWidget, "id" | "result" | "isLoading" | "error" | "lastRefreshed">,
  ) => Promise<{ widgetId: string } | null>;
  onGetDashboard: (dashboardId: string) => DashboardGetResult | null;
  onUpdateWidget: (
    dashboardId: string,
    widgetId: string,
    updates: Partial<DashboardWidget>,
  ) => Promise<void>;
  onRemoveWidget: (dashboardId: string, widgetId: string) => Promise<void>;
}

// --- Config parsing helpers (shared between add_widget and update_widget) ---

function parseChartConfig(raw: unknown): ChartConfig {
  const cc = (typeof raw === "object" && raw !== null ? raw : {}) as Record<string, unknown>;
  const VALID_CHART_TYPES: ChartConfig["type"][] = ["bar", "line", "pie", "scatter", "area"];
  const rawType = typeof cc.type === "string" ? cc.type : "";
  return {
    type: VALID_CHART_TYPES.includes(rawType as ChartConfig["type"])
      ? (rawType as ChartConfig["type"])
      : "bar",
    xAxis: typeof cc.xAxis === "string" ? cc.xAxis : null,
    yAxis: Array.isArray(cc.yAxis)
      ? cc.yAxis.filter((v): v is string => typeof v === "string")
      : [],
    dataScope: "all",
    colors:
      typeof cc.colors === "object" && cc.colors !== null && !Array.isArray(cc.colors)
        ? (cc.colors as Record<string, string>)
        : undefined,
  };
}

function parseKpiConfig(raw: unknown): KpiConfig {
  const kc = (typeof raw === "object" && raw !== null ? raw : {}) as Record<string, unknown>;
  const VALID_FORMATS: KpiConfig["format"][] = ["number", "percentage"];
  const rawFormat = typeof kc.format === "string" ? kc.format : "";
  return {
    label: typeof kc.label === "string" ? kc.label : "",
    valueColumn: typeof kc.valueColumn === "string" ? kc.valueColumn : "",
    format: VALID_FORMATS.includes(rawFormat as KpiConfig["format"])
      ? (rawFormat as KpiConfig["format"])
      : undefined,
    prefix: typeof kc.prefix === "string" ? kc.prefix : undefined,
    suffix: typeof kc.suffix === "string" ? kc.suffix : undefined,
  };
}

function parseTextConfig(raw: unknown): TextConfig {
  const tc = (typeof raw === "object" && raw !== null ? raw : {}) as Record<string, unknown>;
  return { content: typeof tc.content === "string" ? tc.content : "" };
}

// --- Dashboard tool handler ---

export async function handleDashboardToolCall(
  toolName: string,
  input: Record<string, unknown>,
  callbacks: DashboardCallbacks,
): Promise<string> {
  try {
    switch (toolName) {
      case "create_dashboard": {
        const name = typeof input.name === "string" ? input.name : "Untitled Dashboard";
        const result = await callbacks.onCreateDashboard(name);
        if (!result) return JSON.stringify({ error: "Failed to create dashboard" });
        return JSON.stringify({ dashboard_id: result.dashboardId });
      }
      case "add_widget": {
        // oxlint-disable-next-line typescript/no-base-to-string
        const dashboardId = String(input.dashboard_id ?? "");
        const rawWidgetType = typeof input.widget_type === "string" ? input.widget_type : "";
        const VALID_WIDGET_TYPES = ["chart", "kpi", "text"] as const;
        const widgetType: "chart" | "kpi" | "text" = VALID_WIDGET_TYPES.includes(
          rawWidgetType as "chart" | "kpi" | "text",
        )
          ? (rawWidgetType as "chart" | "kpi" | "text")
          : "chart";
        const widget: Omit<
          DashboardWidget,
          "id" | "result" | "isLoading" | "error" | "lastRefreshed"
        > = {
          // oxlint-disable-next-line typescript/no-base-to-string
          title: String(input.title ?? ""),
          x: Number(input.x ?? 0),
          y: Number(input.y ?? 0),
          width: Number(input.width ?? 460),
          height: Number(input.height ?? 340),
          widgetType,
          querySource: "custom",
          query: typeof input.query === "string" ? input.query : "",
          chartConfig:
            widgetType === "chart" && input.chart_config
              ? parseChartConfig(input.chart_config)
              : undefined,
          kpiConfig:
            widgetType === "kpi" && input.kpi_config ? parseKpiConfig(input.kpi_config) : undefined,
          textConfig:
            widgetType === "text" && input.text_config
              ? parseTextConfig(input.text_config)
              : undefined,
        };
        const result = await callbacks.onAddWidget(dashboardId, widget);
        if (!result) return JSON.stringify({ error: "Failed to add widget" });
        return JSON.stringify({ widget_id: result.widgetId });
      }
      case "get_dashboard": {
        // oxlint-disable-next-line typescript/no-base-to-string
        const dashboardId = String(input.dashboard_id ?? "");
        const result = callbacks.onGetDashboard(dashboardId);
        if (!result) return JSON.stringify({ error: "Dashboard not found" });
        return JSON.stringify(result);
      }
      case "update_widget": {
        // oxlint-disable-next-line typescript/no-base-to-string
        const dashboardId = String(input.dashboard_id ?? "");
        // oxlint-disable-next-line typescript/no-base-to-string
        const widgetId = String(input.widget_id ?? "");
        const updates: Partial<DashboardWidget> = {};
        // oxlint-disable-next-line typescript/no-base-to-string
        if (input.title !== undefined) updates.title = String(input.title);
        if (input.x !== undefined) updates.x = Number(input.x);
        if (input.y !== undefined) updates.y = Number(input.y);
        if (input.width !== undefined) updates.width = Number(input.width);
        if (input.height !== undefined) updates.height = Number(input.height);
        if (input.widget_type !== undefined) {
          const wt = typeof input.widget_type === "string" ? input.widget_type : "";
          if (["chart", "kpi", "text"].includes(wt)) {
            updates.widgetType = wt as DashboardWidget["widgetType"];
          }
        }
        // oxlint-disable-next-line typescript/no-base-to-string
        if (input.query !== undefined) updates.query = String(input.query);
        if (input.chart_config !== undefined)
          updates.chartConfig = parseChartConfig(input.chart_config);
        if (input.kpi_config !== undefined) updates.kpiConfig = parseKpiConfig(input.kpi_config);
        if (input.text_config !== undefined)
          updates.textConfig = parseTextConfig(input.text_config);
        await callbacks.onUpdateWidget(dashboardId, widgetId, updates);
        return JSON.stringify({ success: true });
      }
      case "remove_widget": {
        // oxlint-disable-next-line typescript/no-base-to-string
        const dashboardId = String(input.dashboard_id ?? "");
        // oxlint-disable-next-line typescript/no-base-to-string
        const widgetId = String(input.widget_id ?? "");
        await callbacks.onRemoveWidget(dashboardId, widgetId);
        return JSON.stringify({ success: true });
      }
      default:
        return JSON.stringify({ error: "Unknown dashboard tool" });
    }
  } catch (err) {
    return JSON.stringify({ error: err instanceof Error ? err.message : String(err) });
  }
}
