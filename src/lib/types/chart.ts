/**
 * Chart visualization types for query results.
 * @module types/chart
 */

/**
 * Supported chart types for data visualization.
 */
export type ChartType = 'bar' | 'line' | 'pie' | 'scatter';

/**
 * Configuration for rendering a chart from query results.
 */
export interface ChartConfig {
	/** Type of chart to render */
	type: ChartType;
	/** Column name to use for X axis (categories/labels) */
	xAxis: string | null;
	/** Column names to use for Y axis values (can be multiple for grouped charts) */
	yAxis: string[];
	/** Whether to chart all data or just the current page */
	dataScope: 'page' | 'all';
}

/**
 * View mode for displaying query results.
 */
export type ResultViewMode = 'table' | 'chart';
