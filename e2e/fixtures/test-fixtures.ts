/**
 * Playwright test fixtures for Seaquel E2E tests.
 *
 * Provides:
 * - Tauri API mocking
 * - SQLite test database setup/teardown
 * - Common test utilities
 */

import { test as base, expect, type Page } from '@playwright/test';
import {
  createTestDatabase,
  cleanupTestDatabases,
  injectTauriMocks,
  executeQuery,
  clearMockStores,
} from '../mocks/tauri-ipc';

// Test database info
interface TestDatabase {
  path: string;
  connectionString: string;
}

// Extended test fixtures
interface TestFixtures {
  testDb: TestDatabase;
  appPage: Page;
}

/**
 * Extended test with database fixtures
 */
export const test = base.extend<TestFixtures>({
  // Create a fresh test database for each test
  testDb: async ({}, use) => {
    const db = createTestDatabase();
    await use(db);
    // Cleanup happens in afterAll
  },

  // Page with Tauri mocks injected
  appPage: async ({ page, testDb }, use) => {
    // Inject Tauri mocks before navigating
    await injectTauriMocks(page, testDb.connectionString);

    // Navigate to app
    await page.goto('/');

    await use(page);
  },
});

// Re-export expect for convenience
export { expect };

/**
 * Helper to wait for the app to finish loading
 */
export async function waitForAppReady(page: Page): Promise<void> {
  // Wait for the main layout to be visible
  await page.waitForSelector('.app-layout, [data-testid="main-content"]', { timeout: 10000 }).catch(() => {
    // Fallback: just wait for body to have content
  });

  // Small delay for hydration
  await page.waitForTimeout(500);
}

/**
 * Helper to create a new SQLite connection in the app
 */
export async function createConnection(
  page: Page,
  name: string,
  dbPath: string
): Promise<void> {
  // Click add connection button
  await page.getByRole('button', { name: /add|new|connect/i }).first().click();

  // Fill in connection details
  await page.getByLabel(/name/i).fill(name);

  // Select SQLite type if there's a type selector
  const typeSelector = page.getByLabel(/type|database type/i);
  if (await typeSelector.isVisible()) {
    await typeSelector.selectOption('sqlite');
  }

  // Fill in the path/connection string
  const pathInput = page.getByLabel(/path|file|connection string/i);
  if (await pathInput.isVisible()) {
    await pathInput.fill(`sqlite:${dbPath}`);
  }

  // Submit the form
  await page.getByRole('button', { name: /connect|save|add/i }).click();

  // Wait for connection to be established
  await page.waitForTimeout(1000);
}

/**
 * Helper to execute a query in the current tab
 */
export async function executeQueryInEditor(
  page: Page,
  query: string
): Promise<void> {
  // Find the Monaco editor and type the query
  const editor = page.locator('.monaco-editor');
  await editor.click();

  // Clear existing content and type new query
  await page.keyboard.press('Meta+a');
  await page.keyboard.type(query);

  // Execute with keyboard shortcut
  await page.keyboard.press('Meta+Enter');

  // Wait for results
  await page.waitForTimeout(500);
}

/**
 * Helper to get query results from the page
 */
export async function getQueryResults(page: Page): Promise<{
  columns: string[];
  rowCount: number;
}> {
  // Wait for results table
  await page.waitForSelector('table, [data-testid="results-table"]', { timeout: 5000 }).catch(() => null);

  // Get column headers
  const headers = await page.locator('th').allTextContents();

  // Get row count
  const rows = await page.locator('tbody tr').count();

  return {
    columns: headers.filter(h => h.trim()),
    rowCount: rows,
  };
}

/**
 * Helper to check if an error toast appeared
 */
export async function hasErrorToast(page: Page): Promise<boolean> {
  const toast = page.locator('[data-sonner-toast][data-type="error"]');
  return await toast.isVisible({ timeout: 1000 }).catch(() => false);
}

/**
 * Helper to get the active tab name
 */
export async function getActiveTabName(page: Page): Promise<string | null> {
  const activeTab = page.locator('[data-state="active"], .tab-active');
  if (await activeTab.isVisible()) {
    return await activeTab.textContent();
  }
  return null;
}

/**
 * Clean up after all tests
 */
export function setupTestCleanup(): void {
  // Register cleanup for after all tests
  base.afterAll(() => {
    cleanupTestDatabases();
    clearMockStores();
  });
}

// Call setup cleanup by default
setupTestCleanup();
