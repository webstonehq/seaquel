export type ExportFormat = "csv" | "json" | "sql" | "markdown";

export const formatConfig: Record<ExportFormat, { extension: string; name: string }> = {
	csv: { extension: "csv", name: "CSV" },
	json: { extension: "json", name: "JSON" },
	sql: { extension: "sql", name: "SQL" },
	markdown: { extension: "md", name: "Markdown" }
};

export function escapeCSVValue(value: unknown): string {
	if (value === null || value === undefined) return "";
	const str = String(value);
	if (str.includes(",") || str.includes('"') || str.includes("\n")) {
		return `"${str.replace(/"/g, '""')}"`;
	}
	return str;
}

export function escapeSQLValue(value: unknown): string {
	if (value === null || value === undefined) return "NULL";
	if (typeof value === "number") return String(value);
	if (typeof value === "boolean") return value ? "TRUE" : "FALSE";
	const str = String(value);
	return `'${str.replace(/'/g, "''")}'`;
}

export function escapeMarkdownValue(value: unknown): string {
	if (value === null || value === undefined) return "";
	return String(value).replace(/\|/g, "\\|").replace(/\n/g, " ");
}

export function generateCSV(columns: string[], rows: Record<string, unknown>[]): string {
	const header = columns.map(escapeCSVValue).join(",");
	const dataRows = rows.map((row) =>
		columns.map((col) => escapeCSVValue(row[col])).join(",")
	);
	return [header, ...dataRows].join("\n");
}

export function generateJSON(rows: Record<string, unknown>[]): string {
	return JSON.stringify(rows, null, 2);
}

export function generateSQL(
	columns: string[],
	rows: Record<string, unknown>[],
	tableName: string = "table_name"
): string {
	if (rows.length === 0) return "";

	const columnNames = columns.join(", ");
	const inserts = rows.map((row) => {
		const values = columns.map((col) => escapeSQLValue(row[col])).join(", ");
		return `INSERT INTO ${tableName} (${columnNames}) VALUES (${values});`;
	});

	return inserts.join("\n");
}

export function generateMarkdown(columns: string[], rows: Record<string, unknown>[]): string {
	if (rows.length === 0) return "";

	const header = `| ${columns.join(" | ")} |`;
	const separator = `| ${columns.map(() => "---").join(" | ")} |`;
	const dataRows = rows.map(
		(row) => `| ${columns.map((col) => escapeMarkdownValue(row[col])).join(" | ")} |`
	);

	return [header, separator, ...dataRows].join("\n");
}

export function getExportContent(
	format: ExportFormat,
	columns: string[],
	rows: Record<string, unknown>[],
	tableName?: string
): string {
	switch (format) {
		case "csv":
			return generateCSV(columns, rows);
		case "json":
			return generateJSON(rows);
		case "sql":
			return generateSQL(columns, rows, tableName);
		case "markdown":
			return generateMarkdown(columns, rows);
	}
}
