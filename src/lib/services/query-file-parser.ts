/**
 * Parser for .sql files with YAML frontmatter.
 *
 * File format:
 * ```sql
 * ---
 * name: Query Name
 * description: Optional description
 * database: postgresql
 * tags: [tag1, tag2]
 * parameters:
 *   - name: param1
 *     type: number
 *     default: 30
 * ---
 * SELECT * FROM users WHERE id = {{param1}};
 * ```
 */

import type { QueryFrontmatter, SharedQuery, Query, QueryParameter } from "$lib/types";
import { parseYamlValue, parseYamlArray, escapeYamlString } from "./yaml-utils";
import { nameToFilename } from "./config-file-parser.js";

const FRONTMATTER_REGEX = /^---\n([\s\S]*?)\n---\n([\s\S]*)$/;

/**
 * Parses a .sql file with YAML frontmatter into a SharedQuery object.
 * @param folderPrefix - Optional prefix to strip from folder derivation
 *   (e.g., ".seaquel/projects/my-project/queries" so folder shows "analytics" instead of full path)
 */
export function parseQueryFile(
  content: string,
  repoId: string,
  filePath: string,
  folderPrefix?: string,
): SharedQuery | null {
  const match = content.match(FRONTMATTER_REGEX);

  let frontmatter: QueryFrontmatter;
  let query: string;

  if (match) {
    const yamlContent = match[1];
    query = match[2].trim();
    frontmatter = parseYamlFrontmatter(yamlContent);
  } else {
    // No frontmatter - use file name as query name
    query = content.trim();
    const fileName = filePath.split("/").pop() || "untitled";
    const name = fileName.replace(/\.sql$/i, "").replace(/[-_]/g, " ");
    frontmatter = { name };
  }

  // Derive folder from file path, stripping the queries base prefix
  const pathParts = filePath.split("/");
  pathParts.pop(); // Remove filename
  let folder = pathParts.join("/");
  if (folderPrefix && folder.startsWith(folderPrefix)) {
    folder = folder.slice(folderPrefix.length);
    if (folder.startsWith("/")) folder = folder.slice(1);
  }

  return {
    id: `${repoId}:${filePath}`,
    repoId,
    filePath,
    name: frontmatter.name,
    description: frontmatter.description,
    query,
    parameters: frontmatter.parameters,
    databaseType: frontmatter.database,
    tags: frontmatter.tags || [],
    folder,
  };
}

/**
 * Serializes a query to .sql file format with YAML frontmatter.
 * Accepts both the unified Query type and legacy SharedQuery type.
 */
export function serializeQueryFile(query: Query | SharedQuery): string {
  const frontmatter: QueryFrontmatter = {
    name: query.name,
  };

  if (query.description) {
    frontmatter.description = query.description;
  }

  if (query.databaseType) {
    frontmatter.database = query.databaseType;
  }

  if (query.tags && query.tags.length > 0) {
    frontmatter.tags = query.tags;
  }

  if (query.parameters && query.parameters.length > 0) {
    frontmatter.parameters = query.parameters;
  }

  const yamlContent = serializeYamlFrontmatter(frontmatter);
  return `---\n${yamlContent}---\n${query.query}\n`;
}

/**
 * Simple YAML parser for frontmatter (handles our specific format).
 * This avoids adding a full YAML library dependency.
 */
function parseYamlFrontmatter(yaml: string): QueryFrontmatter {
  const result: QueryFrontmatter = { name: "" };
  const lines = yaml.split("\n");

  let inParameters = false;
  let currentParam: Partial<QueryParameter> | null = null;
  const parameters: QueryParameter[] = [];

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const trimmed = line.trim();

    if (!trimmed || trimmed.startsWith("#")) {
      continue;
    }

    // Check for top-level key
    const keyMatch = line.match(/^(\w+):\s*(.*)$/);
    if (keyMatch) {
      const key = keyMatch[1];
      const value = keyMatch[2].trim();

      if (key === "parameters") {
        inParameters = true;
        continue;
      }

      inParameters = false;

      if (key === "name") {
        result.name = parseYamlValue(value);
      } else if (key === "description") {
        result.description = parseYamlValue(value);
      } else if (key === "database") {
        result.database = parseYamlValue(value);
      } else if (key === "tags") {
        result.tags = parseYamlArray(value);
      }

      continue;
    }

    // Handle parameter entries
    if (inParameters) {
      // New parameter item (starts with -)
      const paramStart = line.match(/^\s+-\s*(\w+):\s*(.*)$/);
      if (paramStart) {
        if (currentParam && currentParam.name) {
          parameters.push({
            name: currentParam.name,
            type: currentParam.type || "text",
            defaultValue: currentParam.defaultValue,
            description: currentParam.description,
          });
        }
        currentParam = {};
        const paramKey = paramStart[1];
        const paramValue = paramStart[2].trim();
        setParamField(currentParam, paramKey, paramValue);
        continue;
      }

      // Continuation of current parameter
      const paramContinue = line.match(/^\s+(\w+):\s*(.*)$/);
      if (paramContinue && currentParam) {
        const paramKey = paramContinue[1];
        const paramValue = paramContinue[2].trim();
        setParamField(currentParam, paramKey, paramValue);
      }
    }
  }

  // Don't forget last parameter
  if (currentParam && currentParam.name) {
    parameters.push({
      name: currentParam.name,
      type: currentParam.type || "text",
      defaultValue: currentParam.defaultValue,
      description: currentParam.description,
    });
  }

  if (parameters.length > 0) {
    result.parameters = parameters;
  }

  return result;
}

function setParamField(param: Partial<QueryParameter>, key: string, value: string) {
  const parsed = parseYamlValue(value);
  if (key === "name") {
    param.name = parsed;
  } else if (key === "type") {
    param.type = parsed as QueryParameter["type"];
  } else if (key === "default" || key === "defaultValue") {
    param.defaultValue = parsed;
  } else if (key === "description") {
    param.description = parsed;
  }
}

/**
 * Serialize frontmatter to YAML format.
 */
function serializeYamlFrontmatter(frontmatter: QueryFrontmatter): string {
  const lines: string[] = [];

  lines.push(`name: ${escapeYamlString(frontmatter.name)}`);

  if (frontmatter.description) {
    lines.push(`description: ${escapeYamlString(frontmatter.description)}`);
  }

  if (frontmatter.database) {
    lines.push(`database: ${frontmatter.database}`);
  }

  if (frontmatter.tags && frontmatter.tags.length > 0) {
    const tagsStr = frontmatter.tags.map((t) => escapeYamlString(t)).join(", ");
    lines.push(`tags: [${tagsStr}]`);
  }

  if (frontmatter.parameters && frontmatter.parameters.length > 0) {
    lines.push("parameters:");
    for (const param of frontmatter.parameters) {
      lines.push(`  - name: ${param.name}`);
      lines.push(`    type: ${param.type}`);
      if (param.defaultValue !== undefined) {
        lines.push(`    default: ${escapeYamlString(param.defaultValue)}`);
      }
      if (param.description) {
        lines.push(`    description: ${escapeYamlString(param.description)}`);
      }
    }
  }

  return lines.join("\n") + "\n";
}

/**
 * Generates a valid filename from a query name.
 */
export function queryNameToFilename(name: string): string {
  return `${nameToFilename(name)}.sql`;
}

/** Extracts `{{name}}` parameter names from a query (`$lib/sql`). */
export { extractParameters } from "$lib/sql";

/**
 * Validates that a query file path is valid for the repo.
 */
export function isValidQueryPath(filePath: string): boolean {
  // Must end with .sql
  if (!filePath.toLowerCase().endsWith(".sql")) {
    return false;
  }

  // No path traversal
  if (filePath.includes("..")) {
    return false;
  }

  // No hidden files or directories (except .seaquel config)
  const parts = filePath.split("/");
  for (const part of parts) {
    if (part.startsWith(".") && part !== ".seaquel") {
      return false;
    }
  }

  return true;
}
