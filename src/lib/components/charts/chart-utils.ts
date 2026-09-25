import type { ChartType, ChartConfig } from "$lib/types";
import { SqlDecimal, toNumber } from "$lib/values";

/**
 * Check if a value is numeric (number or numeric string).
 */
function isNumeric(value: unknown): boolean {
  if (typeof value === "number") return !isNaN(value);
  if (typeof value === "bigint") return true;
  if (value instanceof SqlDecimal) return !isNaN(toNumber(value));
  if (typeof value === "string") {
    const trimmed = value.trim();
    if (trimmed === "") return false;
    return !isNaN(Number(trimmed));
  }
  return false;
}

/**
 * X-axis value for layerchart. d3 scales coerce with `+x`, which throws on a
 * bigint, so exact numbers become plain numbers; everything else is kept.
 */
export function chartXValue(value: unknown): unknown {
  return typeof value === "bigint" || value instanceof SqlDecimal ? toNumber(value) : value;
}

/**
 * Analyze column data to determine if it's numeric.
 * Returns true if >80% of non-null values are numeric.
 */
function isNumericColumn(columns: string[], rows: unknown[][], columnName: string): boolean {
  const idx = columns.indexOf(columnName);
  if (idx === -1) return false;
  const values = rows.map((row) => row[idx]).filter((v) => v != null && v !== "");
  if (values.length === 0) return false;

  const numericCount = values.filter(isNumeric).length;
  return numericCount / values.length > 0.8;
}

/**
 * Check if a column contains sequential/ordered data (dates, timestamps, or sequential numbers).
 */
function isSequentialColumn(columns: string[], rows: unknown[][], columnName: string): boolean {
  if (rows.length < 2) return false;

  const idx = columns.indexOf(columnName);
  if (idx === -1) return false;
  const values = rows.map((row) => row[idx]);

  // Check if values are dates
  const isDate = values.every((v) => {
    if (v instanceof Date) return true;
    if (typeof v === "string") {
      const date = new Date(v);
      return !isNaN(date.getTime());
    }
    return false;
  });

  if (isDate) return true;

  // Check if values are sequential numbers
  if (isNumericColumn(columns, rows, columnName)) {
    const nums = values.map(toNumber);
    let increasing = true;
    let decreasing = true;
    for (let i = 1; i < nums.length; i++) {
      if (nums[i] <= nums[i - 1]) increasing = false;
      if (nums[i] >= nums[i - 1]) decreasing = false;
    }
    return increasing || decreasing;
  }

  return false;
}

/**
 * Auto-detect the most appropriate chart type based on data shape.
 */
export function detectChartType(columns: string[], rows: unknown[][]): ChartType {
  if (columns.length === 0 || rows.length === 0) {
    return "bar";
  }

  const numericColumns = columns.filter((col) => isNumericColumn(columns, rows, col));
  const nonNumericColumns = columns.filter((col) => !isNumericColumn(columns, rows, col));

  // 1. Exactly 1 text + 1 numeric column → Pie chart (good for categories)
  if (nonNumericColumns.length === 1 && numericColumns.length === 1 && rows.length <= 10) {
    return "pie";
  }

  // 2. Two numeric columns → Scatter plot
  if (numericColumns.length === 2 && nonNumericColumns.length === 0) {
    return "scatter";
  }

  // 3. Sequential first column + numeric columns → Line chart
  if (
    columns.length >= 2 &&
    isSequentialColumn(columns, rows, columns[0]) &&
    numericColumns.length >= 1
  ) {
    return "line";
  }

  // 4. Default: Bar chart (works well for most data)
  return "bar";
}

/**
 * Suggest appropriate X and Y axis columns based on data.
 */
export function suggestAxes(
  columns: string[],
  rows: unknown[][],
): { xAxis: string | null; yAxis: string[] } {
  if (columns.length === 0) {
    return { xAxis: null, yAxis: [] };
  }

  const numericColumns = columns.filter((col) => isNumericColumn(columns, rows, col));
  const nonNumericColumns = columns.filter((col) => !isNumericColumn(columns, rows, col));

  // If there's a non-numeric column, use it as X axis (categories)
  if (nonNumericColumns.length > 0) {
    return {
      xAxis: nonNumericColumns[0],
      yAxis: numericColumns.slice(0, 3), // Limit to 3 Y columns for clarity
    };
  }

  // All numeric: use first as X, rest as Y
  if (numericColumns.length >= 2) {
    return {
      xAxis: numericColumns[0],
      yAxis: numericColumns.slice(1, 4),
    };
  }

  // Single column: use row index as X
  return {
    xAxis: null,
    yAxis: numericColumns,
  };
}

/**
 * Create a default chart configuration based on data.
 */
export function createDefaultChartConfig(columns: string[], rows: unknown[][]): ChartConfig {
  const chartType = detectChartType(columns, rows);
  const { xAxis, yAxis } = suggestAxes(columns, rows);

  return {
    type: chartType,
    xAxis,
    yAxis,
    dataScope: "page",
  };
}

/**
 * Transform query result rows into chart-ready data.
 */
export function transformDataForChart(
  config: ChartConfig,
  columns: string[],
  rows: unknown[][],
): { labels: string[]; datasets: { label: string; data: number[] }[] } {
  const labels: string[] = [];
  const datasets: { label: string; data: number[] }[] = config.yAxis.map((col) => ({
    label: col,
    data: [],
  }));

  const xIdx = config.xAxis ? columns.indexOf(config.xAxis) : -1;
  const yIdx = config.yAxis.map((col) => columns.indexOf(col));

  rows.forEach((row, index) => {
    // Get label from X axis column or use row index
    // oxlint-disable-next-line typescript/no-base-to-string
    const label =
      xIdx !== -1 ? String((row[xIdx] as string) ?? `Row ${index + 1}`) : `Row ${index + 1}`;
    labels.push(label);

    // Get values for each Y axis column
    yIdx.forEach((idx, colIndex) => {
      const value = idx === -1 ? undefined : row[idx];
      const numValue = typeof value === "number" ? value : toNumber(value) || 0;
      datasets[colIndex].data.push(numValue);
    });
  });

  return { labels, datasets };
}

/**
 * Get chart colors from CSS variables.
 */
export function getChartColors(): string[] {
  return [
    "var(--color-chart-1)",
    "var(--color-chart-2)",
    "var(--color-chart-3)",
    "var(--color-chart-4)",
    "var(--color-chart-5)",
  ];
}
