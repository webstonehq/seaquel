/**
 * Sample dashboard configuration for the browser demo.
 * Creates an "E-Commerce Overview" dashboard with various chart types.
 */

import type { DashboardWidget } from "$lib/types";
import { getLibrary } from "$lib/hooks/database/library/index.js";

const DEMO_WIDGETS: DashboardWidget[] = [
  // === ROW 1: KPI Widgets ===
  {
    id: "widget-demo-kpi-revenue",
    title: "Total Revenue",
    x: 20,
    y: 20,
    width: 220,
    height: 140,
    querySource: "custom",
    query:
      "SELECT SUM(total_amount) as total_revenue, SUM(CASE WHEN status = 'completed' THEN total_amount ELSE 0 END) as completed_revenue FROM demo.orders",
    widgetType: "kpi",
    kpiConfig: {
      label: "Total Revenue",
      valueColumn: "total_revenue",
      format: "number",
      prefix: "$",
    },
  },
  {
    id: "widget-demo-kpi-orders",
    title: "Total Orders",
    x: 260,
    y: 20,
    width: 220,
    height: 140,
    querySource: "custom",
    query: "SELECT COUNT(*) as total_orders FROM demo.orders",
    widgetType: "kpi",
    kpiConfig: {
      label: "Total Orders",
      valueColumn: "total_orders",
      format: "number",
    },
  },
  {
    id: "widget-demo-kpi-customers",
    title: "Customers",
    x: 500,
    y: 20,
    width: 220,
    height: 140,
    querySource: "custom",
    query: "SELECT COUNT(*) as total_customers FROM demo.customers",
    widgetType: "kpi",
    kpiConfig: {
      label: "Customers",
      valueColumn: "total_customers",
      format: "number",
    },
  },
  {
    id: "widget-demo-kpi-avg-order",
    title: "Avg Order Value",
    x: 740,
    y: 20,
    width: 220,
    height: 140,
    querySource: "custom",
    query: "SELECT ROUND(AVG(total_amount), 2) as avg_order_value FROM demo.orders",
    widgetType: "kpi",
    kpiConfig: {
      label: "Avg Order Value",
      valueColumn: "avg_order_value",
      format: "number",
      prefix: "$",
    },
  },

  // === ROW 2: Bar + Pie Charts ===
  {
    id: "widget-demo-bar-revenue",
    title: "Revenue by Product",
    x: 20,
    y: 180,
    width: 460,
    height: 340,
    querySource: "custom",
    query: `SELECT p.name, ROUND(SUM(oi.quantity * oi.unit_price), 2) as revenue
FROM demo.order_items oi
JOIN demo.products p ON oi.product_id = p.id
GROUP BY p.name
ORDER BY revenue DESC`,
    widgetType: "chart",
    chartConfig: {
      type: "bar",
      xAxis: "name",
      yAxis: ["revenue"],
      dataScope: "all",
      colors: { revenue: "#f97316" },
    },
  },
  {
    id: "widget-demo-pie-status",
    title: "Orders by Status",
    x: 500,
    y: 180,
    width: 460,
    height: 340,
    querySource: "custom",
    query: `SELECT
  status,
  COUNT(*) as count
FROM demo.orders
GROUP BY status
ORDER BY count DESC`,
    widgetType: "chart",
    chartConfig: {
      type: "pie",
      xAxis: "status",
      yAxis: ["count"],
      dataScope: "all",
      colors: {
        completed: "#22c55e",
        shipped: "#3b82f6",
        processing: "#f59e0b",
        pending: "#94a3b8",
      },
    },
  },

  // === ROW 3: Area + Scatter Charts ===
  {
    id: "widget-demo-area-monthly",
    title: "Monthly Revenue",
    x: 20,
    y: 540,
    width: 460,
    height: 340,
    querySource: "custom",
    query: `SELECT
  strftime(created_at, '%Y-%m') as month,
  ROUND(SUM(total_amount), 2) as revenue
FROM demo.orders
GROUP BY month
ORDER BY month`,
    widgetType: "chart",
    chartConfig: {
      type: "area",
      xAxis: "month",
      yAxis: ["revenue"],
      dataScope: "all",
      colors: { revenue: "#8b5cf6" },
    },
  },
  {
    id: "widget-demo-scatter-price",
    title: "Product Price vs Units Sold",
    x: 500,
    y: 540,
    width: 460,
    height: 340,
    querySource: "custom",
    query: `SELECT
  p.name,
  p.price,
  COALESCE(SUM(oi.quantity), 0) as units_sold
FROM demo.products p
LEFT JOIN demo.order_items oi ON p.id = oi.product_id
GROUP BY p.name, p.price
ORDER BY p.price`,
    widgetType: "chart",
    chartConfig: {
      type: "scatter",
      xAxis: "name",
      yAxis: ["price", "units_sold"],
      dataScope: "all",
      colors: { price: "#06b6d4", units_sold: "#06b6d4" },
    },
  },

  // === ROW 4: Line chart + Text widget ===
  {
    id: "widget-demo-line-categories",
    title: "Products by Category",
    x: 20,
    y: 900,
    width: 460,
    height: 340,
    querySource: "custom",
    query: `SELECT
  category,
  COUNT(*) as product_count,
  ROUND(AVG(price), 2) as avg_price
FROM demo.products
GROUP BY category
ORDER BY product_count DESC`,
    widgetType: "chart",
    chartConfig: {
      type: "bar",
      xAxis: "category",
      yAxis: ["product_count", "avg_price"],
      dataScope: "all",
      colors: { product_count: "#3b82f6", avg_price: "#f43f5e" },
    },
  },
  {
    id: "widget-demo-line-spending",
    title: "Top Customers by Spending",
    x: 500,
    y: 900,
    width: 460,
    height: 340,
    querySource: "custom",
    query: `SELECT
  c.first_name || ' ' || c.last_name as customer,
  COUNT(o.id) as orders,
  ROUND(SUM(o.total_amount), 2) as total_spent
FROM demo.customers c
JOIN demo.orders o ON c.id = o.customer_id
GROUP BY c.id, c.first_name, c.last_name
ORDER BY total_spent DESC`,
    widgetType: "chart",
    chartConfig: {
      type: "line",
      xAxis: "customer",
      yAxis: ["total_spent"],
      dataScope: "all",
      colors: { total_spent: "#10b981" },
    },
  },
];

/**
 * Whether two dashboard names clash in Core's check. Core compares
 * `name_key` (trim, NFC, full Unicode case folding); for the sample's
 * ASCII name, trimming and lowercasing both sides is the same.
 */
function sameName(a: string, b: string): boolean {
  return a.trim().toLowerCase() === b.trim().toLowerCase();
}

/** The sample dashboard's name. */
export const DEMO_DASHBOARD_NAME = "E-Commerce Overview";

/**
 * The demo's sample dashboard, created on the first load only. Called after
 * the demo connection is established, once the page has loaded the project
 * (its dashboards and its restored tabs).
 *
 * A reload finds it stored (by name, in the library's own list) and only
 * runs its widgets again, since their rows aren't stored and the sample
 * tables were just seeded again; it opens no tab, because the reload
 * restored the tabs the visitor had. Creating it again was refused by
 * Core's name check, an error toast on every reload. A visitor who renamed
 * it gets a new one once, under the sample's name.
 *
 * - Names are compared as Core's check compares them for this name: trimmed
 *   and case-folded (`sameName`), so a sample renamed to
 *   "e-commerce overview" is found, not created again and refused.
 * - It works in the active project: a reload with another project active
 *   creates one sample there.
 * - A failed `listDashboards` throws, and the demo's start shows its
 *   generic "Failed to initialize demo database" toast.
 */
export async function createDemoDashboard(db: {
  state: { activeProjectId: string | null };
  dashboards: {
    createDashboard: (name: string) => Promise<{ id: string } | null>;
    addWidget: (dashboardId: string, widget: DashboardWidget) => Promise<void>;
    executeAllWidgets: (dashboardId: string) => Promise<void>;
  };
  dashboardTabs: {
    add: (dashboardId?: string, dashboardName?: string) => string | null;
  };
}): Promise<void> {
  const projectId = db.state.activeProjectId;
  if (!projectId) return;

  const { value: stored } = await getLibrary().listDashboards(projectId);
  const existing = stored.find((d) => sameName(d.name, DEMO_DASHBOARD_NAME));
  if (existing) {
    await db.dashboards.executeAllWidgets(existing.id);
    return;
  }

  const dashboard = await db.dashboards.createDashboard(DEMO_DASHBOARD_NAME);
  if (!dashboard) return;

  // Add all widgets
  for (const widget of DEMO_WIDGETS) {
    await db.dashboards.addWidget(dashboard.id, widget);
  }

  // Open the dashboard tab
  db.dashboardTabs.add(dashboard.id, DEMO_DASHBOARD_NAME);

  // Execute all widget queries to populate data
  await db.dashboards.executeAllWidgets(dashboard.id);
}
